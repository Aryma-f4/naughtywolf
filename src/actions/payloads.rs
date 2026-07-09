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

/// Generate an implant by calling gen-payload.sh (uses sliver-client + expect).
/// Runs asynchronously, polls for the binary file in ./payloads.
pub async fn generate_implant(
    _conn: &mut SliverConnection,
    req: GeneratePayloadRequest,
) -> GenerateResponse {
    let save_dir = std::env::current_dir().unwrap_or_default().join("payloads");
    std::fs::create_dir_all(&save_dir).ok();
    let beacon_str = if req.is_beacon { "true".to_string() } else { "false".to_string() };
    let port_str = req.lport.to_string();

    // Build the gen-payload.sh command
    let script_path = std::env::var("GEN_PAYLOAD_SCRIPT")
        .unwrap_or_else(|_| "/opt/naughtywolf/gen-payload.sh".to_string());

    let mut child = match tokio::process::Command::new(&script_path)
        .args([
            &req.name, &req.goos, &req.goarch, &req.format,
            &req.protocol, &req.lhost, &port_str,
            &beacon_str,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Failed to run {script_path}: {e}. Ensure gen-payload.sh exists (set GEN_PAYLOAD_SCRIPT)");
            tracing::error!("{msg}");
            return GenerateResponse { success: false, message: msg, implant_name: None, output_path: None };
        }
    };

    // Wait with timeout (10 min for compilation)
    match tokio::time::timeout(std::time::Duration::from_secs(600), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if output.status.success() && stdout.contains("GEN_DONE") {
                // Poll for the binary for up to 60s
                let dl = poll_for_binary(&req.name, &save_dir, 60);
                GenerateResponse {
                    success: true,
                    message: format!("Payload '{}' generated", req.name),
                    implant_name: Some(req.name),
                    output_path: dl.map(|f| format!("/api/payloads/download/{}", f)),
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                GenerateResponse {
                    success: false,
                    message: format!("gen-payload.sh failed: {}", stderr.lines().next().unwrap_or("unknown")),
                    implant_name: None, output_path: None,
                }
            }
        }
        Ok(Err(e)) => GenerateResponse {
            success: false, message: format!("Process error: {e}"),
            implant_name: None, output_path: None,
        },
        Err(_) => GenerateResponse {
            success: false, message: "Payload generation timed out (10 min)".into(),
            implant_name: None, output_path: None,
        },
    }
}

/// Poll the save directory for the generated binary for up to `timeout_secs` seconds.
fn poll_for_binary(name: &str, dir: &std::path::Path, timeout_secs: u64) -> Option<String> {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < timeout_secs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                if (fname.contains(name) || fname.starts_with(name))
                    && e.metadata().map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
                {
                    return Some(fname);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
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
