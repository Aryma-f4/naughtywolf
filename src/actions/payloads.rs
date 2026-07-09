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

/// Generate an implant by spawning sliver-server as a child process with `--rc`.
/// The process times out after 300s to allow first-time Go module downloads.
/// After completion, we scan the save dir and common Sliver paths for the binary.
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

    // Write RC script with save to our payloads dir
    let save_dir = std::env::current_dir().unwrap_or_default().join("payloads");
    let save_str = save_dir.to_string_lossy().to_string();
    std::fs::create_dir_all(&save_dir).ok();

    let rc_cmd = format!(
        "generate --name {} --os {} --arch {} --format {} {} {} --save {}",
        req.name, req.goos, req.goarch, req.format, proto, beacon, save_str
    );

    let rc_path = format!("/tmp/nw_rc_{}.txt", std::process::id());
    let _ = std::fs::write(&rc_path, &rc_cmd);

    let bin_raw = std::env::var("SLIVER_SERVER_PATH").unwrap_or_else(|_| "sliver-server".to_string());
    let parts: Vec<&str> = bin_raw.split_whitespace().collect();
    let (bin_cmd, bin_args) = parts.split_first().unwrap_or((&"sliver-server", &[]));

    let mut child = match tokio::process::Command::new(bin_cmd)
        .args(bin_args)
        .args(["--rc", &rc_path])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Failed to spawn {bin_cmd}: {e}. Set SLIVER_SERVER_PATH");
            tracing::error!("{msg}");
            let _ = std::fs::remove_file(&rc_path);
            return GenerateResponse { success: false, message: msg, implant_name: None, output_path: None };
        }
    };
    let _ = std::fs::remove_file(&rc_path);

    match tokio::time::timeout(std::time::Duration::from_secs(300), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", stdout, stderr);

            if output.status.success() {
                // Scan save_dir + standard sliver locations for the binary
                let out_path = find_binary(&req.name, &save_dir);
                tracing::info!("Payload generated, binary at {:?}", out_path);
                GenerateResponse {
                    success: true,
                    message: format!("Payload '{}' generated", req.name),
                    implant_name: Some(req.name),
                    output_path: out_path.map(|p| format!("/api/payloads/download/{}", p)),
                }
            } else {
                let msg = format!("sliver-server: exit code {}: {}",
                    output.status.code().unwrap_or(-1),
                    &combined[..300.min(combined.len())]
                );
                tracing::error!("{msg}");
                GenerateResponse { success: false, message: msg, implant_name: None, output_path: None }
            }
        }
        Ok(Err(e)) => {
            let msg = format!("sliver-server process error: {e}");
            tracing::error!("{msg}");
            GenerateResponse { success: false, message: msg, implant_name: None, output_path: None }
        }
        Err(_) => {
            let msg = "Payload generation timed out after 300s".to_string();
            tracing::error!("{msg}");
            GenerateResponse { success: false, message: msg, implant_name: None, output_path: None }
        }
    }
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
