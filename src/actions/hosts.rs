use serde::Serialize;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::commonpb;

/// A host extracted from session/beacon data.
#[derive(Debug, Serialize)]
pub struct HostResponse {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub transport: String,
    pub remote_addr: String,
    pub username: String,
    pub agent_count: usize,
    pub agents: Vec<HostAgent>,
}

#[derive(Debug, Serialize)]
pub struct HostAgent {
    pub id: String,
    pub name: String,
    pub r#type: String, // "session" or "beacon"
    pub last_checkin: String,
    pub status: String,
    pub pid: i32,
    pub filename: String,
}

fn ts_to_string(ts: i64) -> String {
    if ts == 0 { return "N/A".to_string(); }
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(dt) => dt.to_rfc3339(),
        None => "N/A".to_string(),
    }
}

/// List all unique hosts from sessions and beacons.
pub async fn list_hosts(conn: &mut SliverConnection) -> Result<Vec<HostResponse>, String> {
    let sessions = conn.client
        .get_sessions(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("get_sessions failed: {e}"))?
        .into_inner().sessions;

    let beacons = conn.client
        .get_beacons(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("get_beacons failed: {e}"))?
        .into_inner().beacons;

    use std::collections::HashMap;
    let mut host_map: HashMap<String, HostResponse> = HashMap::new();

    for s in sessions {
        let hostname = if s.hostname.is_empty() { s.remote_address.clone() } else { s.hostname.clone() };
        let entry = host_map.entry(hostname.clone()).or_insert(HostResponse {
            hostname: hostname.clone(),
            os: s.os.clone(),
            arch: s.arch.clone(),
            transport: s.transport.clone(),
            remote_addr: s.remote_address.clone(),
            username: s.username.clone(),
            agent_count: 0,
            agents: Vec::new(),
        });
        entry.agent_count += 1;
        entry.agents.push(HostAgent {
            id: s.id.clone(),
            name: s.name,
            r#type: "session".to_string(),
            last_checkin: ts_to_string(s.last_checkin),
            status: if s.is_dead { "dead".to_string() } else { "active".to_string() },
            pid: s.pid,
            filename: s.filename,
        });
    }

    for b in beacons {
        let hostname = if b.hostname.is_empty() { b.remote_address.clone() } else { b.hostname.clone() };
        let entry = host_map.entry(hostname.clone()).or_insert(HostResponse {
            hostname: hostname.clone(),
            os: b.os.clone(),
            arch: b.arch.clone(),
            transport: b.transport.clone(),
            remote_addr: b.remote_address.clone(),
            username: b.username.clone(),
            agent_count: 0,
            agents: Vec::new(),
        });
        entry.agent_count += 1;
        entry.agents.push(HostAgent {
            id: b.id.clone(),
            name: b.name,
            r#type: "beacon".to_string(),
            last_checkin: ts_to_string(b.last_checkin),
            status: if b.is_dead { "dead".to_string() } else { "active".to_string() },
            pid: b.pid,
            filename: b.filename,
        });
    }

    let mut hosts: Vec<HostResponse> = host_map.into_values().collect();
    hosts.sort_by(|a, b| b.agent_count.cmp(&a.agent_count));
    Ok(hosts)
}
