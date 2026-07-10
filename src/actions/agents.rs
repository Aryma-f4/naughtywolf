use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::{commonpb, sliverpb};

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

/// Request body for executing a task on a Sliver agent (session or beacon).
#[derive(Debug, Deserialize)]
pub struct TaskRequest {
    /// "shell" to run a shell command, or "execute" to run a program
    pub action: String,
    /// For "shell": the shell command. For "execute": arguments to the program
    pub args: Option<String>,
    /// Path to the executable (required for "execute" action)
    pub exec_path: Option<String>,
}

/// Response from executing a task on a Sliver agent.
#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub success: bool,
    /// Task ID, if the execution was queued (beacon) or returned by the server
    pub task_id: Option<String>,
    /// Combined stdout + stderr output from the command/program
    pub output: Option<String>,
    /// Human-readable status message
    pub message: String,
}

/// Dispatch a task to a Sliver session via the Execute RPC.
pub async fn exec_task(
    conn: &mut SliverConnection,
    session_id: &str,
    req: TaskRequest,
) -> Result<TaskResponse, String> {
    match req.action.as_str() {
        "shell" => exec_shell(conn, session_id, req).await,
        "execute" => exec_program(conn, session_id, req).await,
        _ => Err(format!(
            "Unknown action: '{}'. Use 'shell' or 'execute'.",
            req.action
        )),
    }
}

async fn exec_shell(
    conn: &mut SliverConnection,
    session_id: &str,
    req: TaskRequest,
) -> Result<TaskResponse, String> {
    let command = req.args.unwrap_or_default();

    let execute_req = sliverpb::ExecuteReq {
        path: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), command],
        output: true,
        stdout: String::new(),
        stderr: String::new(),
        env_inheritance: true,
        env: HashMap::new(),
        background: false,
        p_pid: 0,
        request: Some(commonpb::Request {
            r#async: false,
            timeout: 60,
            beacon_id: String::new(),
            session_id: session_id.to_string(),
        }),
    };

    let response = conn
        .client
        .execute(tonic::Request::new(execute_req))
        .await
        .map_err(|e| format!("Execute RPC failed: {e}"))?;

    let exec = response.into_inner();

    let stdout = String::from_utf8_lossy(&exec.stdout).to_string();
    let stderr = String::from_utf8_lossy(&exec.stderr).to_string();

    let output = if stderr.is_empty() {
        stdout
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    Ok(TaskResponse {
        success: exec.status == 0,
        task_id: exec.response.as_ref().and_then(|r| {
            if r.task_id.is_empty() {
                None
            } else {
                Some(r.task_id.clone())
            }
        }),
        output: Some(output),
        message: format!("Shell command exited with status {}", exec.status),
    })
}

async fn exec_program(
    conn: &mut SliverConnection,
    session_id: &str,
    req: TaskRequest,
) -> Result<TaskResponse, String> {
    let path = req
        .exec_path
        .ok_or_else(|| "exec_path is required for 'execute' action".to_string())?;
    let args: Vec<String> = req
        .args
        .map(|a| a.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let execute_req = sliverpb::ExecuteReq {
        path,
        args,
        output: true,
        stdout: String::new(),
        stderr: String::new(),
        env_inheritance: true,
        env: HashMap::new(),
        background: false,
        p_pid: 0,
        request: Some(commonpb::Request {
            r#async: false,
            timeout: 120,
            beacon_id: String::new(),
            session_id: session_id.to_string(),
        }),
    };

    let response = conn
        .client
        .execute(tonic::Request::new(execute_req))
        .await
        .map_err(|e| format!("Execute RPC failed: {e}"))?;

    let exec = response.into_inner();

    let stdout = String::from_utf8_lossy(&exec.stdout).to_string();
    let stderr = String::from_utf8_lossy(&exec.stderr).to_string();

    let output = if stderr.is_empty() {
        stdout
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    Ok(TaskResponse {
        success: exec.status == 0,
        task_id: exec.response.as_ref().and_then(|r| {
            if r.task_id.is_empty() {
                None
            } else {
                Some(r.task_id.clone())
            }
        }),
        output: Some(output),
        message: format!("Program exited with status {}", exec.status),
    })
}

/// List pending/completed tasks for a session.
///
/// Sessions execute commands in real-time and have no stored task history,
/// so this returns an empty vec. For beacons, use the beacon-specific API
/// (GetBeaconTasks) instead.
pub async fn list_session_tasks(
    conn: &mut SliverConnection,
    _session_id: &str,
) -> Result<Vec<TaskResponse>, String> {
    let _ = conn;
    Ok(Vec::new())
}
