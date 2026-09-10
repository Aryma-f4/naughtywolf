use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

use nw_profile::control::CallbackCapabilities;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum OperationStatus {
    Planned,
    Active,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum AssetStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Running)
                | (
                    Self::Running,
                    Self::Succeeded | Self::Failed | Self::Cancelled
                )
        )
    }
}

pub type CheckRunState = RunState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct Operation {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub scope_note: String,
    pub status: OperationStatus,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct Asset {
    pub id: String,
    pub operation_id: String,
    pub name: String,
    pub kind: String,
    pub owner: String,
    pub address: String,
    pub status: AssetStatus,
    pub notes: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct CheckRun {
    pub id: String,
    pub check_id: String,
    pub asset_id: String,
    pub operation_id: String,
    pub requested_by: Option<String>,
    pub state: RunState,
    #[sqlx(json)]
    pub input_json: Value,
    #[sqlx(json(nullable))]
    pub result_json: Option<Value>,
    pub error_summary: Option<String>,
    pub output_truncated: bool,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct Evidence {
    pub id: String,
    pub check_run_id: String,
    pub storage_path: String,
    pub content_type: String,
    pub byte_len: i64,
    pub sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct AuditEvent {
    pub id: String,
    pub actor_id: Option<String>,
    pub operation_id: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub parameter_summary: Option<String>,
    pub outcome: String,
    pub correlation_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum CallbackStatus {
    Active,
    Beacon,
    Dormant,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct Callback {
    pub id: String,
    pub asset_id: Option<String>,
    pub operation_id: Option<String>,
    pub host: String,
    pub user_name: String,
    pub process: String,
    pub arch: String,
    pub os: String,
    pub os_version: Option<String>,
    pub executable_path: Option<String>,
    pub local_addr: Option<String>,
    pub implant_version: Option<String>,
    pub interval_ms: Option<i64>,
    pub jitter_ms: Option<i64>,
    #[sqlx(json(nullable))]
    pub capabilities_json: Option<CallbackCapabilities>,
    pub protocol: String,
    pub status: CallbackStatus,
    pub last_seen: String,
    pub created_at: String,
}

impl Callback {
    /// A callback is online while it remains within three reported beacon
    /// intervals, with a 30-second floor for short-interval or legacy agents.
    pub fn is_online(&self, now: time::OffsetDateTime) -> bool {
        let Ok(last_seen) = chrono::DateTime::parse_from_rfc3339(&self.last_seen) else {
            return false;
        };
        let interval_ms = self.interval_ms.unwrap_or_default().max(0) as i128;
        let online_window_ms = interval_ms.saturating_mul(3).max(30_000);
        let now_ms = now.unix_timestamp_nanos() / 1_000_000;
        let elapsed_ms = now_ms - i128::from(last_seen.timestamp_millis());

        elapsed_ms <= online_window_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Delivering,
    Delivered,
    Processing,
    Completed,
    Error,
    Cancelled,
}

impl TaskStatus {
    /// Mythic-style human-readable label + CSS class for task state display.
    pub fn label_class(&self) -> (&'static str, &'static str) {
        match self {
            Self::Pending => ("Submitted", "neutral"),
            Self::Delivering => ("Delivering", "neutral"),
            Self::Delivered => ("Delivered", "warning"),
            Self::Processing => ("Processing", "warning"),
            Self::Completed => ("Completed", "success"),
            Self::Error => ("Error", "danger"),
            Self::Cancelled => ("Cancelled", "danger"),
        }
    }

    /// Parse a status string (from DB) and return the Mythic-style label + CSS class.
    pub fn label_class_from_str(status: &str) -> (&'static str, &'static str) {
        match status {
            "pending" => ("Submitted", "neutral"),
            "delivering" => ("Delivering", "neutral"),
            "delivered" => ("Delivered", "warning"),
            "processing" => ("Processing", "warning"),
            "completed" => ("Completed", "success"),
            "error" => ("Error", "danger"),
            "cancelled" => ("Cancelled", "danger"),
            _ => ("Unknown", "neutral"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct C2Task {
    pub id: String,
    pub session_id: String,
    pub command: String,
    #[sqlx(json)]
    pub args_json: Value,
    pub timeout_ms: u64,
    pub status: String,
    pub created_at: String,
    pub processing_at: Option<String>,
    pub completed_at: Option<String>,
    pub result_output: Option<String>,
    pub result_ok: Option<bool>,
    pub result_exit_code: Option<i32>,
    pub operator_id: Option<String>,
    pub parent_task_id: Option<String>,
    pub updated_at: String,
    pub cancellation_requested_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct C2TaskWithResult {
    pub id: String,
    pub session_id: String,
    pub command: String,
    #[sqlx(json)]
    pub args_json: Value,
    pub status: String,
    pub created_at: String,
    pub processing_at: Option<String>,
    pub completed_at: Option<String>,
    pub result_output: Option<String>,
    pub result_ok: Option<bool>,
    pub result_exit_code: Option<i32>,
    pub result_stderr: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProcessSnapshot {
    pub session_id: String,
    pub task_id: String,
    pub schema_version: String,
    #[sqlx(json)]
    pub snapshot_json: Value,
    pub captured_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileSnapshot {
    pub session_id: String,
    pub path: String,
    pub task_id: String,
    pub schema_version: String,
    #[sqlx(json)]
    pub snapshot_json: Value,
    pub captured_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileTransfer {
    pub id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub direction: String,
    pub remote_path: String,
    pub storage_key: String,
    pub expected_size: Option<i64>,
    pub received_bytes: i64,
    pub sha256: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct EventRule {
    pub id: String,
    pub name: String,
    pub trigger: String,
    pub command: String,
    pub target: String,
    pub enabled: bool,
    pub requested_by: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
pub struct InstalledService {
    pub id: String,
    pub asset_id: Option<String>,
    pub operation_id: Option<String>,
    pub name: String,
    pub install_path: String,
    pub running: bool,
    pub started_at: Option<String>,
    pub created_at: String,
}
