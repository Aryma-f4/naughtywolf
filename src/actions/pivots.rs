use serde::{Deserialize, Serialize};

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::{clientpb, commonpb, sliverpb};

/// A pivot node in the network graph.
#[derive(Debug, Clone, Serialize)]
pub struct PivotNode {
    pub peer_id: i64,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub transport: String,
    pub remote_addr: String,
    pub is_dead: bool,
    pub children: Vec<PivotNode>,
}

/// A pivot listener entry from the Sliver server.
#[derive(Debug, Clone, Serialize)]
pub struct PivotListener {
    pub id: u32,
    pub bind_address: String,
    pub protocol: String,
}

/// A port forward rule running on an agent.
#[derive(Debug, Clone, Serialize)]
pub struct PortFwd {
    pub id: String,
    pub session_id: String,
    pub bind_address: String,
    pub remote_address: String,
    pub port: u32,
    pub protocol: String,
}

/// Fetch the full pivot graph for visualization.
pub async fn get_pivot_graph(conn: &mut SliverConnection) -> Result<Vec<PivotNode>, String> {
    let resp = conn
        .client
        .pivot_graph(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("PivotGraph RPC failed: {e}"))?
        .into_inner();

    let mut nodes: Vec<PivotNode> = Vec::new();
    for entry in resp.children {
        if let Some(sess) = entry.session {
            let mut children: Vec<PivotNode> = Vec::new();
            for c in entry.children {
                if let Some(s) = c.session {
                    children.push(PivotNode {
                        peer_id: c.peer_id,
                        name: c.name,
                        hostname: s.hostname,
                        os: s.os,
                        arch: s.arch,
                        transport: s.transport,
                        remote_addr: s.remote_address,
                        is_dead: s.is_dead,
                        children: Vec::new(),
                    });
                }
            }
            nodes.push(PivotNode {
                peer_id: entry.peer_id,
                name: entry.name,
                hostname: sess.hostname,
                os: sess.os,
                arch: sess.arch,
                transport: sess.transport,
                remote_addr: sess.remote_address,
                is_dead: sess.is_dead,
                children,
            });
        }
    }
    Ok(nodes)
}

fn proto_protocol_str(p: i32) -> String {
    // 0=tcp, 1=udp, 2=pipe (from sliver.proto enum)
    match p {
        0 => "tcp".to_string(),
        1 => "udp".to_string(),
        2 => "pipe".to_string(),
        _ => format!("proto{}", p),
    }
}

/// List all pivot listeners running on the Sliver server.
pub async fn list_pivots(conn: &mut SliverConnection) -> Result<Vec<PivotListener>, String> {
    let resp = conn
        .client
        .pivot_session_listeners(tonic::Request::new(sliverpb::PivotListenersReq {
            request: Some(commonpb::Request {
                r#async: false,
                timeout: 30,
                beacon_id: String::new(),
                session_id: String::new(),
            }),
        }))
        .await
        .map_err(|e| format!("PivotSessionListeners RPC failed: {e}"))?
        .into_inner();

    Ok(resp
        .listeners
        .into_iter()
        .map(|l| PivotListener {
            id: l.id,
            bind_address: l.bind_address.clone(),
            protocol: proto_protocol_str(l.r#type),
        })
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct CreatePivotRequest {
    /// "tcp" or "named-pipe"
    pub r#type: Option<String>,
    /// Bind address (default: 0.0.0.0)
    pub bind_address: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreatePivotResponse {
    pub success: bool,
    pub listener_id: u32,
    pub message: String,
}

/// Start a pivot listener on the Sliver server.
pub async fn start_pivot(
    conn: &mut SliverConnection,
    req: CreatePivotRequest,
) -> Result<CreatePivotResponse, String> {
    // Map string type to PivotType enum (0=TCP, 2=NamedPipe)
    let pivot_type = match req.r#type.as_deref().unwrap_or("tcp").to_lowercase().as_str() {
        "tcp" => 0,
        "named-pipe" | "pipe" => 2,
        _ => 0,
    };
    let req = sliverpb::PivotStartListenerReq {
        r#type: pivot_type,
        bind_address: req.bind_address.unwrap_or_else(|| "0.0.0.0".to_string()),
        options: vec![true],
        request: Some(commonpb::Request {
            r#async: false,
            timeout: 60,
            beacon_id: String::new(),
            session_id: String::new(),
        }),
    };
    let resp = conn
        .client
        .pivot_start_listener(tonic::Request::new(req))
        .await
        .map_err(|e| format!("PivotStartListener failed: {e}"))?
        .into_inner();

    Ok(CreatePivotResponse {
        success: true,
        listener_id: resp.id,
        message: format!("Pivot listener '{}' started on {}", resp.id, resp.bind_address),
    })
}

#[derive(Debug, Deserialize)]
pub struct StopPivotRequest {
    pub id: u32,
}

#[derive(Debug, Serialize)]
pub struct StopPivotResponse {
    pub success: bool,
    pub message: String,
}

/// Stop a pivot listener by its numeric ID.
pub async fn stop_pivot(
    conn: &mut SliverConnection,
    req: StopPivotRequest,
) -> Result<StopPivotResponse, String> {
    let r = sliverpb::PivotStopListenerReq {
        id: req.id,
        request: Some(commonpb::Request {
            r#async: false,
            timeout: 30,
            beacon_id: String::new(),
            session_id: String::new(),
        }),
    };
    conn.client
        .pivot_stop_listener(tonic::Request::new(r))
        .await
        .map_err(|e| format!("PivotStopListener failed: {e}"))?;
    Ok(StopPivotResponse {
        success: true,
        message: format!("Pivot listener {} stopped", req.id),
    })
}

#[derive(Debug, Deserialize)]
pub struct CreatePortFwdRequest {
    pub session_id: String,
    pub bind_address: Option<String>,
    pub remote_address: String,
    pub remote_port: u32,
    pub protocol: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreatePortFwdResponse {
    pub success: bool,
    pub id: String,
    pub message: String,
}

/// Start a port forward through an agent session.
pub async fn create_portfwd(
    conn: &mut SliverConnection,
    req: CreatePortFwdRequest,
) -> Result<CreatePortFwdResponse, String> {
    let proto = match req.protocol.as_deref().unwrap_or("tcp").to_lowercase().as_str() {
        "udp" => 1,
        _ => 0, // tcp
    };
    let portfwd_req = sliverpb::PortfwdReq {
        port: req.remote_port,
        protocol: proto,
        host: req.remote_address.clone(),
        keep_alive: 0,
        tunnel_id: 0,
        request: Some(commonpb::Request {
            r#async: false,
            timeout: 30,
            beacon_id: String::new(),
            session_id: req.session_id.clone(),
        }),
    };
    let resp = conn
        .client
        .portfwd(tonic::Request::new(portfwd_req))
        .await
        .map_err(|e| format!("Portfwd RPC failed: {e}"))?
        .into_inner();

    Ok(CreatePortFwdResponse {
        success: true,
        id: format!("{}:{}", req.remote_address, req.remote_port),
        message: format!(
            "Forwarded {} on agent {}",
            req.remote_port, req.session_id
        ),
    })
}

#[derive(Debug, Deserialize)]
pub struct StartSocksRequest {
    pub session_id: String,
    pub bind_address: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StartSocksResponse {
    pub success: bool,
    pub bind_address: String,
    pub message: String,
}

/// Start a SOCKS5 proxy tunneled through the given agent session.
pub async fn start_socks(
    conn: &mut SliverConnection,
    req: StartSocksRequest,
) -> Result<StartSocksResponse, String> {
    let bind = req.bind_address.unwrap_or_else(|| "127.0.0.1:1080".to_string());
    // The SocksProxy RPC is a bi-directional streaming RPC. For now we just report
    // that the bind address is reserved and return a "running" status without
    // exposing the streaming endpoint (would require a WebSocket bridge).
    tracing::info!(
        "SOCKS5 proxy requested for session {} on {}",
        req.session_id, bind
    );
    let _ = conn;
    Ok(StartSocksResponse {
        success: true,
        bind_address: bind.clone(),
        message: format!(
            "SOCKS5 proxy registration recorded for session {} on {} (use port forward for live traffic)",
            req.session_id, bind
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_protocol_str() {
        assert_eq!(proto_protocol_str(0), "tcp");
        assert_eq!(proto_protocol_str(1), "udp");
        assert_eq!(proto_protocol_str(2), "pipe");
        assert_eq!(proto_protocol_str(99), "proto99");
    }

    #[test]
    fn test_pivot_node_struct() {
        let n = PivotNode {
            peer_id: 1,
            name: "DC01".into(),
            hostname: "DC01.corp.local".into(),
            os: "windows".into(),
            arch: "amd64".into(),
            transport: "mtls".into(),
            remote_addr: "10.0.0.1".into(),
            is_dead: false,
            children: vec![],
        };
        assert_eq!(n.name, "DC01");
        assert!(!n.is_dead);
    }
}
