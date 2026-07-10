use serde::Serialize;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::commonpb;

/// Unified representation of a session or beacon agent.
#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub username: String,
    pub transport: String,
    pub remote_addr: String,
    pub os: String,
    pub arch: String,
    /// "session" or "beacon"
    pub r#type: String,
    pub last_checkin: String,
    /// Interval in seconds (0 for sessions)
    pub interval: i64,
    /// "active" or "dead"
    pub status: String,
    pub is_dead: bool,
}

fn ts_to_string(ts: i64) -> String {
    if ts == 0 {
        return "N/A".to_string();
    }
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(dt) => dt.to_rfc3339(),
        None => "N/A".to_string(),
    }
}

/// Fetch all sessions and beacons from the Sliver server, merging them into a
/// unified agent list.
pub async fn list_agents(conn: &mut SliverConnection) -> Result<Vec<AgentResponse>, String> {
    let sessions_resp = conn
        .client
        .get_sessions(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("get_sessions failed: {e}"))?;

    let beacons_resp = conn
        .client
        .get_beacons(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("get_beacons failed: {e}"))?;

    let sessions = sessions_resp.into_inner().sessions;
    let beacons = beacons_resp.into_inner().beacons;

    let mut agents = Vec::with_capacity(sessions.len() + beacons.len());

    for s in sessions {
        agents.push(AgentResponse {
            id: s.id,
            name: s.name,
            hostname: s.hostname,
            username: s.username,
            transport: s.transport,
            remote_addr: s.remote_address,
            os: s.os,
            arch: s.arch,
            r#type: "session".to_string(),
            last_checkin: ts_to_string(s.last_checkin),
            interval: 0,
            status: if s.is_dead { "dead".to_string() } else { "active".to_string() },
            is_dead: s.is_dead,
        });
    }

    for b in beacons {
        agents.push(AgentResponse {
            id: b.id,
            name: b.name,
            hostname: b.hostname,
            username: b.username,
            transport: b.transport,
            remote_addr: b.remote_address,
            os: b.os,
            arch: b.arch,
            r#type: "beacon".to_string(),
            last_checkin: ts_to_string(b.last_checkin),
            interval: b.interval,
            status: if b.is_dead { "dead".to_string() } else { "active".to_string() },
            is_dead: b.is_dead,
        });
    }

    Ok(agents)
}
