use serde::Serialize;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::{clientpb, commonpb};

/// A simplified job representation for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct JobResponse {
    pub id: u32,
    pub name: String,
    pub protocol: String,
    pub port: u32,
    pub description: String,
    pub domains: Vec<String>,
    pub profile_name: String,
}

/// Fetch all active jobs (listeners) from the Sliver server.
pub async fn list_jobs(conn: &mut SliverConnection) -> Result<Vec<JobResponse>, String> {
    let response = conn
        .client
        .get_jobs(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("get_jobs failed: {e}"))?;

    let jobs = response.into_inner();

    Ok(jobs
        .active
        .into_iter()
        .map(|j| JobResponse {
            id: j.id,
            name: j.name,
            protocol: j.protocol,
            port: j.port,
            description: j.description,
            domains: j.domains,
            profile_name: j.profile_name,
        })
        .collect())
}

/// Start a new mTLS listener on the given host:port.
pub async fn start_mtls(
    conn: &mut SliverConnection,
    host: &str,
    port: u32,
) -> Result<clientpb::ListenerJob, String> {
    let req = clientpb::MtlsListenerReq {
        host: host.to_string(),
        port,
    };

    let response = conn
        .client
        .start_mtls_listener(tonic::Request::new(req))
        .await
        .map_err(|e| format!("start_mtls_listener failed: {e}"))?;

    Ok(response.into_inner())
}

/// Start a new HTTP listener on the given host:port with an optional domain.
pub async fn start_http(
    conn: &mut SliverConnection,
    host: &str,
    port: u32,
    domain: &str,
) -> Result<clientpb::ListenerJob, String> {
    let req = clientpb::HttpListenerReq {
        domain: domain.to_string(),
        host: host.to_string(),
        port,
        secure: false,
        website: String::new(),
        cert: Vec::new(),
        key: Vec::new(),
        acme: false,
        enforce_otp: false,
        long_poll_timeout: 0,
        long_poll_jitter: 0,
        randomize_jarm: false,
    };

    let response = conn
        .client
        .start_http_listener(tonic::Request::new(req))
        .await
        .map_err(|e| format!("start_http_listener failed: {e}"))?;

    Ok(response.into_inner())
}

/// Kill a job (listener) by its numeric job ID.
pub async fn kill_job(conn: &mut SliverConnection, job_id: u32) -> Result<(), String> {
    let req = clientpb::KillJobReq { id: job_id };

    conn.client
        .kill_job(tonic::Request::new(req))
        .await
        .map_err(|e| format!("kill_job failed: {e}"))?;

    Ok(())
}
