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
    pub output_path: Option<String>,
}

/// Generate an implant via the Sliver daemon's gRPC `Generate` RPC.
/// Falls back to generating with inline ImplantConfig if no profile exists.
/// On success, saves the binary to `./payloads` for download.
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

    let implant_name = req.name.clone();
    let profile_name = format!("nw_{}", Uuid::new_v4().to_string().split('-').next().unwrap_or("x"));

    // Step 1: Save an implant profile
    let profile_config = clientpb::ImplantConfig {
        id: format!("nw-cfg-{}", Uuid::new_v4()),
        goos: req.goos.clone(),
        goarch: req.goarch.clone(),
        format: format.into(),
        is_beacon: req.is_beacon,
        debug: false,
        obfuscate_symbols: true,
        sgn_enabled: true,
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
        ..Default::default()
    };

    // Save profile, then GenerateStage
    let profile_save = conn.client.save_implant_profile(tonic::Request::new(
        clientpb::ImplantProfile {
            name: profile_name.clone(),
            config: Some(profile_config),
            ..Default::default()
        }
    )).await;

    match profile_save {
        Ok(_) => tracing::info!("Profile '{}' saved", profile_name),
        Err(e) => tracing::warn!("Save profile failed (non-fatal): {}", e),
    }

    // Step 2: GenerateStage uses the profile
    let gen_result = conn.client.generate_stage(tonic::Request::new(
        clientpb::GenerateStageReq {
            profile: profile_name,
            name: format!("implant-{}", implant_name),
            prepend_size: false,
            ..Default::default()
        }
    )).await;

    match gen_result {
        Ok(response) => {
            let out = response.into_inner();
            tracing::info!("Payload generated: {} (build_id: {})", out.implant_name, out.implant_build_id);

            // Save binary to ./payloads for download
            let mut dl_path = None;
            if let Some(file) = out.file {
                let save_dir = std::env::current_dir().unwrap_or_default().join("payloads");
                let _ = std::fs::create_dir_all(&save_dir);
                let fpath = save_dir.join(&out.implant_name);
                if std::fs::write(&fpath, &file.data).is_ok() {
                    dl_path = Some(format!("/api/payloads/download/{}", out.implant_name));
                    tracing::info!("Binary saved: {:?} ({} bytes)", fpath, file.data.len());
                }
            }

            GenerateResponse {
                success: true,
                message: format!("Payload '{}' generated", out.implant_name),
                implant_name: Some(out.implant_name),
                output_path: dl_path,
            }
        }
        Err(e) => {
            tracing::error!("Generate failed: {}", e);
            GenerateResponse {
                success: false,
                message: format!("Generate failed: {}", e),
                implant_name: None,
                output_path: None,
            }
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
