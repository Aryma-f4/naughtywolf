use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

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
