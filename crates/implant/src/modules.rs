use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use nw_profile::msgs::{Task, TaskResult};
use reqwest::{Client, StatusCode, Url};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::runner;

pub const MAX_SOURCE_BYTES: usize = 24 * 1024;
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

const LINPEAS_URL: &str =
    "https://github.com/peass-ng/PEASS-ng/releases/latest/download/linpeas.sh";
const WINPEAS_URL: &str =
    "https://github.com/peass-ng/PEASS-ng/releases/latest/download/winPEASany_ofs.exe";

pub fn is_module_command(command: &str) -> bool {
    matches!(command, "nw/user-enum" | "nw/peas-audit" | "nw/exec-code")
}

pub async fn execute(task: &Task) -> Option<TaskResult> {
    let result = match task.command.as_str() {
        "nw/user-enum" => user_enum(task).await,
        "nw/peas-audit" => peas_audit(task).await,
        "nw/exec-code" => exec_code(task).await,
        _ => return None,
    };
    Some(result)
}

fn failure(task: &Task, message: impl Into<String>) -> TaskResult {
    TaskResult {
        task_id: task.id,
        ok: false,
        stdout: Vec::new(),
        stderr: message.into().into_bytes(),
        exit_code: -1,
    }
}

pub(crate) fn decode_source(encoded: &str) -> Result<String, String> {
    if encoded.len() > MAX_SOURCE_BYTES.div_ceil(3) * 4 {
        return Err("custom source exceeds the 24 KiB limit".into());
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "source must be valid base64".to_owned())?;
    if decoded.len() > MAX_SOURCE_BYTES {
        return Err("custom source exceeds the 24 KiB limit".into());
    }
    String::from_utf8(decoded).map_err(|_| "custom source must be UTF-8".into())
}

pub(crate) fn interpreter(
    language: &str,
    os: &str,
) -> Result<(&'static str, Vec<&'static str>, &'static str), String> {
    match (language, os) {
        ("shell", "linux" | "macos") => Ok(("/bin/sh", Vec::new(), "sh")),
        ("python", "linux" | "macos") => Ok(("python3", Vec::new(), "py")),
        ("python", "windows") => Ok(("python.exe", Vec::new(), "py")),
        ("powershell", "windows") => Ok((
            "powershell.exe",
            vec!["-NoProfile", "-NonInteractive", "-File"],
            "ps1",
        )),
        _ => Err(format!("unsupported language {language:?} on {os}")),
    }
}

fn temp_path(extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!("nw-module-{}.{}", uuid::Uuid::new_v4(), extension))
}

async fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o700);
    }
    let mut file = options
        .open(path)
        .await
        .map_err(|e| format!("create temporary module: {e}"))?;
    file.write_all(data)
        .await
        .map_err(|e| format!("write temporary module: {e}"))
}

async fn drain_capped<R>(mut reader: R, limit: usize) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut kept = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(remaining)]);
            }
        }
    }
    kept
}

async fn run_bounded(task: Task) -> TaskResult {
    use tokio::process::Command;
    let timeout_ms = if task.timeout_ms == 0 {
        30_000
    } else {
        task.timeout_ms
    };
    let mut command = Command::new(&task.command);
    command
        .args(&task.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failure(&task, format!("failed to spawn: {error}")),
    };
    let stdout_task = tokio::spawn(drain_capped(child.stdout.take().unwrap(), MAX_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(drain_capped(child.stderr.take().unwrap(), MAX_OUTPUT_BYTES));
    let waited = tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await;
    let (ok, exit_code, timed_out) = match waited {
        Ok(Ok(status)) => (status.success(), status.code().unwrap_or(-1), false),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            return failure(&task, format!("failed to await: {error}"));
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (false, -1, true)
        }
    };
    let stdout = stdout_task.await.unwrap_or_default();
    let mut stderr = stderr_task.await.unwrap_or_default();
    if timed_out {
        stderr.extend_from_slice(format!("\ntask timed out after {timeout_ms}ms").as_bytes());
    }
    let (stdout, stderr) = bound_output(stdout, stderr);
    TaskResult {
        task_id: task.id,
        ok,
        stdout,
        stderr,
        exit_code,
    }
}

async fn exec_code(task: &Task) -> TaskResult {
    let [language, encoded] = task.args.as_slice() else {
        return failure(
            task,
            "usage: nw/exec-code <shell|powershell|python> <base64-source>",
        );
    };
    let source = match decode_source(encoded) {
        Ok(source) => source,
        Err(error) => return failure(task, error),
    };
    let (program, prefix, extension) = match interpreter(language, std::env::consts::OS) {
        Ok(choice) => choice,
        Err(error) => return failure(task, error),
    };
    let path = temp_path(extension);
    if let Err(error) = write_private(&path, source.as_bytes()).await {
        return failure(task, error);
    }
    let mut args: Vec<String> = prefix.into_iter().map(str::to_owned).collect();
    args.push(path.to_string_lossy().into_owned());
    let result = run_bounded(Task {
        id: task.id,
        command: program.into(),
        args,
        timeout_ms: task.timeout_ms,
    })
    .await;
    let _ = tokio::fs::remove_file(path).await;
    bounded_result(result)
}

pub(crate) fn parse_passwd(contents: &str) -> serde_json::Value {
    let users = contents
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split(':').collect();
            if fields.len() < 7 {
                return None;
            }
            let shell = fields[6];
            Some(json!({
                "username": fields[0],
                "uid": fields[2].parse::<u32>().ok(),
                "gid": fields[3].parse::<u32>().ok(),
                "home": fields[5],
                "shell": shell,
                "login_enabled": !(shell.ends_with("nologin") || shell.ends_with("false"))
            }))
        })
        .collect::<Vec<_>>();
    json!({ "platform": "unix", "users": users })
}

async fn user_enum(task: &Task) -> TaskResult {
    if !task.args.is_empty() {
        return failure(task, "usage: nw/user-enum");
    }
    #[cfg(unix)]
    let result = match tokio::fs::read_to_string("/etc/passwd").await {
        Ok(contents) => {
            let mut value = parse_passwd(&contents);
            let who = runner::run(Task {
                id: task.id,
                command: "who".into(),
                args: Vec::new(),
                timeout_ms: 5_000,
            })
            .await;
            value["current_user"] =
                json!(std::env::var("USER").unwrap_or_else(|_| "unknown".into()));
            value["logged_in"] = json!(
                String::from_utf8_lossy(&who.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            );
            Ok(value)
        }
        Err(error) => Err(format!("read /etc/passwd: {error}")),
    };
    #[cfg(windows)]
    let result = {
        let script =
            "Get-LocalUser | Select-Object Name,Enabled,LastLogon,SID | ConvertTo-Json -Compress";
        let run = runner::run(Task {
            id: task.id,
            command: "powershell.exe".into(),
            args: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script.into(),
            ],
            timeout_ms: task.timeout_ms,
        })
        .await;
        if run.ok {
            serde_json::from_slice::<serde_json::Value>(&run.stdout)
                .map_err(|e| format!("parse user inventory: {e}"))
        } else {
            return bounded_result(run);
        }
    };
    match result {
        Ok(value) => TaskResult {
            task_id: task.id,
            ok: true,
            stdout: serde_json::to_vec_pretty(&value).unwrap_or_default(),
            stderr: Vec::new(),
            exit_code: 0,
        },
        Err(error) => failure(task, error),
    }
}

pub(crate) fn peas_asset(os: &str) -> Result<&'static str, String> {
    match os {
        "linux" => Ok(LINPEAS_URL),
        "windows" => Ok(WINPEAS_URL),
        _ => Err(format!("PEASS assessment is not available for {os}")),
    }
}

pub(crate) fn allowed_download_host(host: &str) -> bool {
    matches!(
        host,
        "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com"
    )
}

async fn download_official(url: &str) -> Result<(Vec<u8>, Url), String> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("build PEASS downloader: {e}"))?;
    let mut current = Url::parse(url).map_err(|e| format!("invalid PEASS URL: {e}"))?;
    for _ in 0..6 {
        let host = current.host_str().unwrap_or_default();
        if !allowed_download_host(host) {
            return Err(format!("blocked PEASS download host: {host}"));
        }
        let response = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| format!("download PEASS: {e}"))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or("PEASS redirect omitted Location")?
                .to_str()
                .map_err(|_| "PEASS redirect Location is invalid")?;
            current = current
                .join(location)
                .map_err(|e| format!("invalid PEASS redirect: {e}"))?;
            continue;
        }
        if response.status() != StatusCode::OK {
            return Err(format!(
                "PEASS download returned HTTP {}",
                response.status()
            ));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_ARTIFACT_BYTES as u64)
        {
            return Err("PEASS artifact exceeds the 64 MiB limit".into());
        }
        let mut response = response;
        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(0)
                .min(MAX_ARTIFACT_BYTES as u64) as usize,
        );
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("read PEASS artifact: {e}"))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_ARTIFACT_BYTES {
                return Err("PEASS artifact exceeds the 64 MiB limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((bytes, current));
    }
    Err("too many PEASS download redirects".into())
}

pub(crate) fn extract_cves(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let upper = token.to_ascii_uppercase();
        let mut parts = upper.split('-');
        let valid = parts.next() == Some("CVE")
            && parts
                .next()
                .is_some_and(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_digit()))
            && parts
                .next()
                .is_some_and(|p| p.len() >= 4 && p.chars().all(|c| c.is_ascii_digit()))
            && parts.next().is_none();
        if valid && !found.contains(&upper) {
            found.push(upper);
        }
    }
    found
}

async fn peas_audit(task: &Task) -> TaskResult {
    if !task.args.is_empty() {
        return failure(task, "usage: nw/peas-audit");
    }
    let url = match peas_asset(std::env::consts::OS) {
        Ok(url) => url,
        Err(error) => return failure(task, error),
    };
    let (artifact, final_url) = match download_official(url).await {
        Ok(value) => value,
        Err(error) => return failure(task, error),
    };
    let digest = format!("{:x}", Sha256::digest(&artifact));
    let extension = if cfg!(windows) { "exe" } else { "sh" };
    let path = temp_path(extension);
    if let Err(error) = write_private(&path, &artifact).await {
        return failure(task, error);
    }
    #[cfg(unix)]
    let (program, args) = (
        "/bin/sh".to_owned(),
        vec![path.to_string_lossy().into_owned()],
    );
    #[cfg(windows)]
    let (program, args) = (
        path.to_string_lossy().into_owned(),
        vec!["systeminfo".into(), "userinfo".into(), "notcolor".into()],
    );
    let run = run_bounded(Task {
        id: task.id,
        command: program,
        args,
        timeout_ms: if task.timeout_ms == 0 {
            300_000
        } else {
            task.timeout_ms
        },
    })
    .await;
    let _ = tokio::fs::remove_file(path).await;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let cves = extract_cves(&text);
    let mut stdout = format!("NaughtyWolf PEASS assessment\nsource: {final_url}\nsha256: {digest}\nautomatic exploitation: disabled\ncve_candidates: {}\n\n", if cves.is_empty() { "none".into() } else { cves.join(", ") }).into_bytes();
    stdout.extend_from_slice(&run.stdout);
    bounded_result(TaskResult { stdout, ..run })
}

pub(crate) fn bound_output(mut stdout: Vec<u8>, mut stderr: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    let total = stdout.len().saturating_add(stderr.len());
    if total <= MAX_OUTPUT_BYTES {
        return (stdout, stderr);
    }
    const NOTICE: &[u8] = b"\n[NaughtyWolf: output truncated at 2 MiB]";
    let capacity = MAX_OUTPUT_BYTES.saturating_sub(NOTICE.len());
    let stdout_keep = stdout.len().min(capacity);
    stdout.truncate(stdout_keep);
    stderr.truncate(capacity.saturating_sub(stdout_keep));
    stderr.extend_from_slice(NOTICE);
    (stdout, stderr)
}

fn bounded_result(mut result: TaskResult) -> TaskResult {
    (result.stdout, result.stderr) = bound_output(result.stdout, result.stderr);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_module_commands() {
        assert!(is_module_command("nw/user-enum"));
        assert!(is_module_command("nw/peas-audit"));
        assert!(is_module_command("nw/exec-code"));
        assert!(!is_module_command("whoami"));
    }

    #[test]
    fn decodes_utf8_source_with_a_hard_limit() {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode("echo halo 🐺");
        assert_eq!(decode_source(&encoded).unwrap(), "echo halo 🐺");
        let oversized =
            base64::engine::general_purpose::STANDARD.encode(vec![b'a'; MAX_SOURCE_BYTES + 1]);
        assert!(decode_source(&oversized).unwrap_err().contains("24 KiB"));
    }

    #[test]
    fn maps_supported_interpreters_without_a_shell_wrapper() {
        assert_eq!(interpreter("shell", "linux").unwrap().0, "/bin/sh");
        assert_eq!(interpreter("python", "linux").unwrap().0, "python3");
        assert_eq!(
            interpreter("powershell", "windows").unwrap().0,
            "powershell.exe"
        );
        assert!(interpreter("ruby", "linux").is_err());
        assert!(interpreter("shell", "windows").is_err());
    }

    #[test]
    fn extracts_unique_cve_references() {
        let refs = extract_cves("CVE-2024-1234 and CVE-2024-1234; cve-2025-99999");
        assert_eq!(refs, vec!["CVE-2024-1234", "CVE-2025-99999"]);
    }

    #[test]
    fn truncates_combined_output_with_a_notice() {
        let (stdout, stderr) = bound_output(vec![b'a'; MAX_OUTPUT_BYTES], vec![b'b'; 4]);
        assert!(stdout.len() + stderr.len() <= MAX_OUTPUT_BYTES + 96);
        assert!(String::from_utf8_lossy(&stderr).contains("truncated"));
    }

    #[test]
    fn parses_unix_accounts_as_structured_json() {
        let value = parse_passwd(
            "root:x:0:0:root:/root:/bin/bash\nsvc:x:998:998::/srv:/usr/sbin/nologin\n",
        );
        let users = value["users"].as_array().unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0]["username"], "root");
        assert_eq!(users[1]["login_enabled"], false);
    }

    #[test]
    fn selects_official_peass_assets_and_restricts_redirect_hosts() {
        assert!(peas_asset("linux").unwrap().ends_with("/linpeas.sh"));
        assert!(
            peas_asset("windows")
                .unwrap()
                .ends_with("/winPEASany_ofs.exe")
        );
        assert!(peas_asset("freebsd").is_err());
        assert!(allowed_download_host("github.com"));
        assert!(allowed_download_host(
            "release-assets.githubusercontent.com"
        ));
        assert!(!allowed_download_host("example.org"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executes_custom_shell_source_and_keeps_the_task_id() {
        use base64::Engine as _;
        let task = Task {
            id: uuid::Uuid::new_v4(),
            command: "nw/exec-code".into(),
            args: vec![
                "shell".into(),
                base64::engine::general_purpose::STANDARD.encode("printf module-ok"),
            ],
            timeout_ms: 5_000,
        };
        let result = execute(&task).await.unwrap();
        assert!(result.ok, "{}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(result.task_id, task.id);
        assert_eq!(result.stdout, b"module-ok");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn custom_code_obeys_the_task_timeout() {
        use base64::Engine as _;
        let task = Task {
            id: uuid::Uuid::new_v4(),
            command: "nw/exec-code".into(),
            args: vec![
                "shell".into(),
                base64::engine::general_purpose::STANDARD.encode("sleep 1"),
            ],
            timeout_ms: 10,
        };
        let result = execute(&task).await.unwrap();
        assert!(!result.ok);
        assert!(String::from_utf8_lossy(&result.stderr).contains("timed out"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn user_enumeration_returns_accounts_and_current_user() {
        let task = Task {
            id: uuid::Uuid::new_v4(),
            command: "nw/user-enum".into(),
            args: Vec::new(),
            timeout_ms: 5_000,
        };
        let result = execute(&task).await.unwrap();
        assert!(result.ok, "{}", String::from_utf8_lossy(&result.stderr));
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert!(!value["users"].as_array().unwrap().is_empty());
        assert!(value["current_user"].is_string());
    }
}
