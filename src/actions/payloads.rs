use serde::{Deserialize, Serialize};

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::commonpb;

/// A simplified implant build representation for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct ImplantBuildResponse {
    pub name: String,
    pub is_beacon: bool,
    pub goos: String,
    pub goarch: String,
    pub debug: bool,
    pub format: i32,
    pub template_name: String,
    pub include_mtls: bool,
    pub include_http: bool,
    pub include_wg: bool,
    pub include_dns: bool,
    pub beacon_interval: i64,
    pub beacon_jitter: i64,
    pub is_staged: bool,
}

/// Fetch all implant builds from the Sliver server.
pub async fn list_builds(
    conn: &mut SliverConnection,
) -> Result<Vec<ImplantBuildResponse>, String> {
    let response = conn
        .client
        .implant_builds(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("implant_builds failed: {e}"))?;

    let builds = response.into_inner();

    Ok(builds
        .configs
        .into_iter()
        .map(|(name, config)| {
            let is_staged = builds.staged.get(&name).copied().unwrap_or(false);
            ImplantBuildResponse {
                name,
                is_beacon: config.is_beacon,
                goos: config.goos,
                goarch: config.goarch,
                debug: config.debug,
                format: config.format,
                template_name: config.template_name,
                include_mtls: config.include_mtls,
                include_http: config.include_http,
                include_wg: config.include_wg,
                include_dns: config.include_dns,
                beacon_interval: config.beacon_interval,
                beacon_jitter: config.beacon_jitter,
                is_staged,
            }
        })
        .collect())
}

#[derive(Deserialize)]
pub struct GeneratePayloadRequest {
    pub name: String,
    pub goos: String,
    pub goarch: String,
    pub format: String,       // "exe" | "shared" | "shellcode" | "service"
    pub is_beacon: bool,
    pub protocol: String,     // "mtls" | "http" | "https" | "dns"
    pub lhost: String,
    pub lport: u16,
}

#[derive(Serialize)]
pub struct GenerateResponse {
    pub success: bool,
    pub message: String,
    pub implant_name: Option<String>,
    pub output_path: Option<String>,
}

/// Generate an implant via sliver-server CLI with `--rc`.
/// The process spawns sliver-server non-interactively and runs `generate`.
pub async fn generate_implant(
    _conn: &mut SliverConnection,
    req: GeneratePayloadRequest,
) -> GenerateResponse {
    let proto = match req.protocol.as_str() {
        "mtls" => format!("--mtls {}:{}", req.lhost, req.lport),
        "http" => format!("--http {}:{}", req.lhost, req.lport),
        "https" => format!("--https {}:{}", req.lhost, req.lport),
        "dns" => format!("--dns {}:{}", req.lhost, req.lport),
        _ => format!("--mtls {}:{}", req.lhost, req.lport),
    };
    let beacon = if req.is_beacon { " --beacon" } else { "" };

    let save_dir = std::env::current_dir().unwrap_or_default().join("payloads");
    let save_str = save_dir.to_string_lossy().to_string();
    std::fs::create_dir_all(&save_dir).ok();

    let rc_cmd = format!(
        "generate --name {} --os {} --arch {} --format {} {} {} --save {}\nexit",
        req.name, req.goos, req.goarch, req.format, proto, beacon, save_str
    );

    let bin_raw = std::env::var("SLIVER_SERVER_PATH").unwrap_or_else(|_| "sliver-server".to_string());
    let parts: Vec<&str> = bin_raw.split_whitespace().collect();
    let (bin_cmd, bin_args) = parts.split_first().unwrap_or((&"sliver-server", &[]));

    let rc_path = format!("/tmp/nw_rc_{}.txt", std::process::id());
    let _ = std::fs::write(&rc_path, &rc_cmd);

    let output = match tokio::process::Command::new(bin_cmd)
        .args(bin_args)
        .args(["--rc", &rc_path])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => {
            let _ = std::fs::remove_file(&rc_path);
            match tokio::time::timeout(std::time::Duration::from_secs(300), c.wait_with_output()).await {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => return GenerateResponse {
                    success: false,
                    message: "Generation timed out after 300s (first compile may take long)".into(),
                    implant_name: None, output_path: None,
                },
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(&rc_path);
            return GenerateResponse {
                success: false,
                message: format!("Failed to start sliver-server (set SLIVER_SERVER_PATH): {e}"),
                implant_name: None, output_path: None,
            };
        }
    };

    let output = match output {
        Ok(o) => o,
        Err(e) => return GenerateResponse { success: false, message: e, implant_name: None, output_path: None },
    };

    if output.status.success() {
        // Scan the save dir for the binary
        let dl = scan_save_dir(&save_dir).map(|f| format!("/api/payloads/download/{}", f));
        GenerateResponse {
            success: true,
            message: format!("Payload '{}' generated", req.name),
            implant_name: Some(req.name),
            output_path: dl,
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{}{}", &stdout[..100.min(stdout.len())], &stderr[..200.min(stderr.len())]);
        GenerateResponse {
            success: false,
            message: format!("sliver-server failed: {}", combined),
            implant_name: None, output_path: None,
        }
    }
}

/// Scan directory for binary file matching our naming convention.
fn scan_save_dir(dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with('.') && e.metadata().map(|m| m.is_file()).unwrap_or(false) {
            return Some(name);
        }
    }
    None
}

/// Search for the generated binary in the save dir, returning the filename.
fn find_binary(name: &str, dir: &std::path::Path) -> Option<String> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            if fname.contains(name) || fname.starts_with(name) {
                return Some(fname);
            }
        }
    }
    None
}

/// Scan `./payloads` directory for locally-generated implant files.
pub async fn list_local_payloads() -> Result<Vec<ImplantBuildResponse>, String> {
    let dir = std::path::Path::new("./payloads");
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("Failed to read payloads dir: {e}"))?;

    let mut builds = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Error reading dir entry: {e}"))?
    {
        if entry.metadata().await.map(|m| m.is_file()).unwrap_or(false) {
            let name = entry.file_name().to_string_lossy().trim().to_string();
            builds.push(ImplantBuildResponse {
                name,
                is_beacon: false,
                goos: "local".into(),
                goarch: "local".into(),
                debug: false,
                format: 2, // EXECUTABLE
                template_name: "cli-generated".into(),
                include_mtls: false,
                include_http: false,
                include_wg: false,
                include_dns: false,
                beacon_interval: 0,
                beacon_jitter: 0,
                is_staged: false,
            });
        }
    }
    Ok(builds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_response_struct() {
        let r = ImplantBuildResponse {
            name: "test".into(),
            is_beacon: true,
            goos: "linux".into(),
            goarch: "amd64".into(),
            debug: false,
            format: 0,
            template_name: "".into(),
            include_mtls: true,
            include_http: false,
            include_wg: false,
            include_dns: false,
            beacon_interval: 60,
            beacon_jitter: 30,
            is_staged: false,
        };
        assert_eq!(r.name, "test");
        assert!(r.is_beacon);
        assert_eq!(r.goos, "linux");
        assert_eq!(r.goarch, "amd64");
        assert_eq!(r.format, 0);
        assert_eq!(r.beacon_interval, 60);
    }
}
