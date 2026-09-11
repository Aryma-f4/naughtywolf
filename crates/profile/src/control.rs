use serde::{Deserialize, Serialize};

pub const PROCESS_LIST_SCHEMA_V1: &str = "nw.process-list.v1";
pub const PROCESS_KILL_SCHEMA_V1: &str = "nw.process-kill.v1";
pub const FILE_LIST_SCHEMA_V1: &str = "nw.fs-list.v1";
pub const FILE_MUTATION_SCHEMA_V1: &str = "nw.fs-mutation.v1";

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub modified_at: Option<String>,
    pub permissions: Option<String>,
    pub owner: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileListV1 {
    pub schema: String,
    pub captured_at: String,
    pub path: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMutationV1 {
    pub schema: String,
    pub action: String,
    pub path: String,
    pub destination: Option<String>,
    pub recursive: bool,
    pub completed_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum FileControlError {
    #[error("path was not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("path already exists")]
    AlreadyExists,
    #[error("directory is not empty")]
    DirectoryNotEmpty,
    #[error("path is invalid")]
    InvalidPath,
    #[error("filesystem operation failed")]
    IoFailure,
}

impl FileControlError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::AlreadyExists => "already_exists",
            Self::DirectoryNotEmpty => "directory_not_empty",
            Self::InvalidPath => "invalid_path",
            Self::IoFailure => "io_failure",
        }
    }
}

/// Normalize an absolute remote path without consulting the local filesystem.
/// This deliberately understands both Windows and Unix roots so the portal can
/// key snapshots for callbacks running on a different OS.
pub fn normalize_remote_path(path: &str) -> Result<String, FileControlError> {
    if path.is_empty() || path.contains('\0') {
        return Err(FileControlError::InvalidPath);
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        let root = format!("{}\\", &path[..2]);
        return normalize_components(&root, &path[3..], true);
    }
    if path.starts_with(r"\\") {
        let mut root_parts = path
            .trim_start_matches(['/', '\\'])
            .split(['/', '\\'])
            .filter(|part| !part.is_empty());
        let server = root_parts.next().ok_or(FileControlError::InvalidPath)?;
        let share = root_parts.next().ok_or(FileControlError::InvalidPath)?;
        if matches!(server, "." | "..") || matches!(share, "." | "..") {
            return Err(FileControlError::InvalidPath);
        }
        let root = format!(r"\\{server}\{share}");
        return normalize_parts(&root, root_parts, '\\');
    }
    if let Some(remainder) = path.strip_prefix('/') {
        return normalize_components("/", remainder, false);
    }
    Err(FileControlError::InvalidPath)
}

fn normalize_components(
    root: &str,
    remainder: &str,
    windows: bool,
) -> Result<String, FileControlError> {
    if windows {
        normalize_parts(
            root,
            remainder.split(['/', '\\']).filter(|part| !part.is_empty()),
            '\\',
        )
    } else {
        normalize_parts(
            root,
            remainder.split('/').filter(|part| !part.is_empty()),
            '/',
        )
    }
}

fn normalize_parts<'a>(
    root: &str,
    parts: impl Iterator<Item = &'a str>,
    separator: char,
) -> Result<String, FileControlError> {
    let mut normalized = Vec::new();
    for part in parts {
        match part {
            "." => {}
            ".." => {
                normalized.pop();
            }
            part if part.contains('\0') => return Err(FileControlError::InvalidPath),
            part => normalized.push(part),
        }
    }
    if normalized.is_empty() {
        return Ok(root.to_owned());
    }
    let mut result = root.to_owned();
    if !result.ends_with(separator) {
        result.push(separator);
    }
    result.push_str(&normalized.join(&separator.to_string()));
    Ok(result)
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
    use super::{CallbackCapabilities, normalize_remote_path};

    #[test]
    fn legacy_capabilities_default_to_disabled() {
        let capabilities = CallbackCapabilities::default();

        assert!(!capabilities.process_browser);
        assert!(!capabilities.file_browser);
        assert!(!capabilities.file_transfer);
        assert!(!capabilities.task_ack);
    }

    #[test]
    fn control_remote_path_normalization_preserves_linux_drive_and_unc_roots() {
        assert_eq!(normalize_remote_path("/").unwrap(), "/");
        assert_eq!(
            normalize_remote_path("/srv//lab/../files/").unwrap(),
            "/srv/files"
        );
        assert_eq!(normalize_remote_path(r"C:\").unwrap(), r"C:\");
        assert_eq!(
            normalize_remote_path(r"C:\Temp\.\files\..").unwrap(),
            r"C:\Temp"
        );
        assert_eq!(
            normalize_remote_path(r"\\server\share\folder\..").unwrap(),
            r"\\server\share"
        );
    }

    #[test]
    fn control_remote_path_normalization_rejects_relative_empty_and_nul_paths() {
        for path in ["", "relative/path", "bad\0path", r"C:relative"] {
            assert!(normalize_remote_path(path).is_err(), "{path:?}");
        }
    }
}
