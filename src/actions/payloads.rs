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
}

/// Generate an implant by spawning sliver-server as a child process with `--rc`.
/// This avoids gRPC "record not found" issues and works with the local sliver-server binary.
pub async fn generate_implant(
    req: GeneratePayloadRequest,
) -> GenerateResponse {
    let proto = match req.protocol.as_str() {
        "mtls" => format!("--mtls {}:{}", req.lhost, req.lport),
        "http" => format!("--http {}:{}", req.lhost, req.lport),
        "https" => format!("--https {}:{}", req.lhost, req.lport),
        "dns" => format!("--dns {}:{}", req.lhost, req.lport),
        _ => format!("--mtls {}:{}", req.lhost, req.lport),
    };
    let beacon_flag = if req.is_beacon { " --beacon" } else { "" };
    let save_dir = "./payloads";
    std::fs::create_dir_all(save_dir).ok();

    let cmd = format!(
        "generate --name {} --os {} --arch {} --format {} {} {} --save {}\nexit",
        req.name, req.goos, req.goarch, req.format, proto, beacon_flag, save_dir
    );

    tracing::info!("Running sliver-server generate: {}", &cmd[..cmd.find('\n').unwrap_or(cmd.len())]);

    let bin_raw = std::env::var("SLIVER_SERVER_PATH").unwrap_or_else(|_| "sliver-server".to_string());
    let parts: Vec<&str> = bin_raw.split_whitespace().collect();
    let bin_cmd = parts[0];
    let bin_args: Vec<&str> = parts[1..].iter().map(|s| *s).collect();

    let mut command = tokio::process::Command::new(bin_cmd);
    command
        .args(&bin_args)
        .args(["--rc", "/dev/stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let result = command.spawn();

    let mut child = match result {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Failed to spawn {bin_cmd}: {e}. Set SLIVER_SERVER_PATH or install sliver-server");
            tracing::error!("{msg}");
            return GenerateResponse { success: false, message: msg, implant_name: None };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(cmd.as_bytes()).await.ok();
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(180),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.success() {
                tracing::info!("Payload generated successfully");
                GenerateResponse {
                    success: true,
                    message: format!("Payload '{}' generated", req.name),
                    implant_name: Some(req.name),
                }
            } else {
                let msg = format!("sliver-server exited with code {}: {}",
                    output.status.code().unwrap_or(-1),
                    if !stderr.is_empty() { &stderr[..200.min(stderr.len())] } else { &stdout[..200.min(stdout.len())] }
                );
                tracing::error!("{}", msg);
                GenerateResponse { success: false, message: msg, implant_name: None }
            }
        }
        Ok(Err(e)) => {
            let msg = format!("sliver-server process error: {e}");
            tracing::error!("{}", msg);
            GenerateResponse { success: false, message: msg, implant_name: None }
        }
        Err(_) => {
            let msg = "sliver-server timed out after 180s".to_string();
            tracing::error!("{}", msg);
            GenerateResponse { success: false, message: msg, implant_name: None }
        }
    }
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
