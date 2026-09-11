use serde::{Deserialize, Serialize};

pub const PROCESS_LIST_SCHEMA_V1: &str = "nw.process-list.v1";
pub const PROCESS_KILL_SCHEMA_V1: &str = "nw.process-kill.v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub executable: Option<String>,
    pub user: Option<String>,
    pub architecture: Option<String>,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub started_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessListV1 {
    pub schema: String,
    pub captured_at: String,
    pub processes: Vec<ProcessEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessKillV1 {
    pub schema: String,
    pub pid: u32,
    pub name: String,
    pub terminated: bool,
    pub terminated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ControlError {
    #[error("process {pid} does not exist")]
    NoSuchProcess { pid: u32 },
    #[error("permission denied terminating process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("termination failed for process {pid}")]
    TerminateFailed { pid: u32 },
}

impl ControlError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoSuchProcess { .. } => "no_such_process",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::TerminateFailed { .. } => "terminate_failed",
        }
    }
}

/// Workspace features understood by a callback. Older registrations omit this
/// object and therefore deserialize to a conservative, all-disabled value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallbackCapabilities {
    #[serde(default)]
    pub process_browser: bool,
    #[serde(default)]
    pub file_browser: bool,
    #[serde(default)]
    pub file_transfer: bool,
    #[serde(default)]
    pub task_ack: bool,
}

#[cfg(test)]
mod tests {
    use super::CallbackCapabilities;

    #[test]
    fn legacy_capabilities_default_to_disabled() {
        let capabilities = CallbackCapabilities::default();

        assert!(!capabilities.process_browser);
        assert!(!capabilities.file_browser);
        assert!(!capabilities.file_transfer);
        assert!(!capabilities.task_ack);
    }
}
