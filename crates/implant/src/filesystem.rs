use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, SecondsFormat, Utc};
use nw_profile::{
    control::{
        FILE_LIST_SCHEMA_V1, FILE_MUTATION_SCHEMA_V1, FileControlError, FileEntry, FileListV1,
        FileMutationV1,
    },
    msgs::{Task, TaskResult},
};

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn map_error(error: io::Error) -> FileControlError {
    match error.kind() {
        io::ErrorKind::NotFound => FileControlError::NotFound,
        io::ErrorKind::PermissionDenied => FileControlError::PermissionDenied,
        io::ErrorKind::AlreadyExists => FileControlError::AlreadyExists,
        io::ErrorKind::DirectoryNotEmpty => FileControlError::DirectoryNotEmpty,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => FileControlError::InvalidPath,
        _ => FileControlError::IoFailure,
    }
}

fn normalized(path: &str) -> Result<PathBuf, FileControlError> {
    let result = PathBuf::from(nw_profile::control::normalize_remote_path(path)?);
    if !result.is_absolute() {
        return Err(FileControlError::InvalidPath);
    }
    Ok(result)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn modified_at(value: Result<SystemTime, io::Error>) -> Option<String> {
    value
        .ok()
        .map(DateTime::<Utc>::from)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[cfg(unix)]
fn permissions(metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    Some(format!("{:04o}", metadata.permissions().mode() & 0o7777))
}

#[cfg(unix)]
fn owner(metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.uid().to_string())
}

#[cfg(not(unix))]
fn owner(_metadata: &fs::Metadata) -> Option<String> {
    None
}

#[cfg(windows)]
fn permissions(metadata: &fs::Metadata) -> Option<String> {
    Some(
        if metadata.permissions().readonly() {
            "read-only"
        } else {
            "read-write"
        }
        .to_owned(),
    )
}

#[cfg(not(any(unix, windows)))]
fn permissions(_metadata: &fs::Metadata) -> Option<String> {
    None
}

fn entry(path: &Path) -> Result<FileEntry, FileControlError> {
    let metadata = fs::symlink_metadata(path).map_err(map_error)?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        "directory"
    } else if file_type.is_file() {
        "file"
    } else if file_type.is_symlink() {
        "symlink"
    } else {
        "other"
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_string(path));
    Ok(FileEntry {
        name,
        path: path_string(path),
        kind: kind.to_owned(),
        size: metadata.len(),
        modified_at: modified_at(metadata.modified()),
        permissions: permissions(&metadata),
        owner: owner(&metadata),
    })
}

pub fn list(path: &str) -> Result<FileListV1, FileControlError> {
    let path = normalized(path)?;
    let mut entries = fs::read_dir(&path)
        .map_err(map_error)?
        .map(|entry_result| {
            let entry_path = entry_result.map_err(map_error)?.path();
            entry(&entry_path)
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(FileListV1 {
        schema: FILE_LIST_SCHEMA_V1.to_owned(),
        captured_at: now(),
        path: path_string(&path),
        entries,
    })
}

pub fn stat(path: &str) -> Result<FileEntry, FileControlError> {
    let path = normalized(path)?;
    entry(&path)
}

pub fn mkdir(path: &str) -> Result<FileMutationV1, FileControlError> {
    let path = normalized(path)?;
    fs::create_dir(&path).map_err(map_error)?;
    Ok(mutation("mkdir", &path, None, false))
}

pub fn move_path(source: &str, destination: &str) -> Result<FileMutationV1, FileControlError> {
    let source = normalized(source)?;
    let destination = normalized(destination)?;
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(FileControlError::AlreadyExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(map_error(error)),
    }
    fs::rename(&source, &destination).map_err(map_error)?;
    Ok(mutation("move", &source, Some(&destination), false))
}

pub fn delete(path: &str, recursive: bool) -> Result<FileMutationV1, FileControlError> {
    let path = normalized(path)?;
    let metadata = fs::symlink_metadata(&path).map_err(map_error)?;
    if metadata.file_type().is_dir() {
        if recursive {
            fs::remove_dir_all(&path).map_err(map_error)?;
        } else {
            if fs::read_dir(&path).map_err(map_error)?.next().is_some() {
                return Err(FileControlError::DirectoryNotEmpty);
            }
            fs::remove_dir(&path).map_err(map_error)?;
        }
    } else {
        fs::remove_file(&path).map_err(map_error)?;
    }
    Ok(mutation("delete", &path, None, recursive))
}

fn mutation(
    action: &str,
    path: &Path,
    destination: Option<&Path>,
    recursive: bool,
) -> FileMutationV1 {
    FileMutationV1 {
        schema: FILE_MUTATION_SCHEMA_V1.to_owned(),
        action: action.to_owned(),
        path: path_string(path),
        destination: destination.map(path_string),
        recursive,
        completed_at: now(),
    }
}

fn failure(task: &Task, code: &str, message: impl Into<String>) -> TaskResult {
    let stderr = serde_json::to_vec(&serde_json::json!({
        "code": code,
        "message": message.into(),
    }))
    .unwrap_or_else(|_| b"{\"code\":\"serialization_failed\"}".to_vec());
    TaskResult {
        task_id: task.id,
        ok: false,
        stdout: Vec::new(),
        stderr,
        exit_code: -1,
    }
}

pub fn execute(task: &Task) -> Option<TaskResult> {
    let result = match task.command.as_str() {
        "nw/fs-list" => match task.args.as_slice() {
            [path] => list(path).and_then(serialize),
            _ => return Some(invalid_arguments(task, "usage: nw/fs-list <absolute-path>")),
        },
        "nw/fs-stat" => match task.args.as_slice() {
            [path] => stat(path).and_then(serialize),
            _ => return Some(invalid_arguments(task, "usage: nw/fs-stat <absolute-path>")),
        },
        "nw/fs-mkdir" => match task.args.as_slice() {
            [path] => mkdir(path).and_then(serialize),
            _ => {
                return Some(invalid_arguments(
                    task,
                    "usage: nw/fs-mkdir <absolute-path>",
                ));
            }
        },
        "nw/fs-move" => match task.args.as_slice() {
            [source, destination] => move_path(source, destination).and_then(serialize),
            _ => {
                return Some(invalid_arguments(
                    task,
                    "usage: nw/fs-move <source> <destination>",
                ));
            }
        },
        "nw/fs-delete" => match task.args.as_slice() {
            [path, recursive] if recursive == "false" || recursive == "true" => {
                delete(path, recursive == "true").and_then(serialize)
            }
            _ => {
                return Some(invalid_arguments(
                    task,
                    "usage: nw/fs-delete <absolute-path> <recursive:false|true>",
                ));
            }
        },
        _ => return None,
    };
    Some(match result {
        Ok(stdout) => TaskResult {
            task_id: task.id,
            ok: true,
            stdout,
            stderr: Vec::new(),
            exit_code: 0,
        },
        Err(error) => failure(task, error.code(), error.to_string()),
    })
}

fn serialize(value: impl serde::Serialize) -> Result<Vec<u8>, FileControlError> {
    serde_json::to_vec(&value).map_err(|_| FileControlError::IoFailure)
}

fn invalid_arguments(task: &Task, usage: &str) -> TaskResult {
    failure(task, "invalid_arguments", usage)
}
