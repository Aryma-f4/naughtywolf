use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, SecondsFormat, Utc};
use nw_profile::{
    control::{
        FILE_LIST_SCHEMA_V1, FILE_MUTATION_SCHEMA_V1, FileControlError, FileEntry, FileListV1,
        FileMutationV1, MAX_FILE_LIST_BYTES, MAX_FILE_LIST_ENTRIES,
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
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData | io::ErrorKind::NotADirectory => {
            FileControlError::InvalidPath
        }
        _ if platform_invalid_path(&error) => FileControlError::InvalidPath,
        _ => FileControlError::IoFailure,
    }
}

#[cfg(unix)]
fn platform_invalid_path(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(code) if matches!(code, libc::ENOTDIR | libc::ENAMETOOLONG | libc::ELOOP))
}

#[cfg(windows)]
fn platform_invalid_path(error: &io::Error) -> bool {
    // ERROR_INVALID_NAME, ERROR_BAD_PATHNAME, ERROR_FILENAME_EXCED_RANGE,
    // ERROR_DIRECTORY and ERROR_CANT_RESOLVE_FILENAME.
    matches!(error.raw_os_error(), Some(123 | 161 | 206 | 267 | 1921))
}

#[cfg(not(any(unix, windows)))]
fn platform_invalid_path(_error: &io::Error) -> bool {
    false
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
    let mut entries = Vec::new();
    for entry_result in fs::read_dir(&path).map_err(map_error)? {
        if entries.len() == MAX_FILE_LIST_ENTRIES {
            return Err(FileControlError::ResultTooLarge);
        }
        let entry_path = entry_result.map_err(map_error)?.path();
        entries.push(entry(&entry_path)?);
    }
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
    rename_no_replace(&source, &destination).map_err(map_error)?;
    Ok(mutation("move", &source, Some(&destination), false))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: both C strings remain alive for this call and are NUL terminated.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: both C strings remain alive for this call and are NUL terminated.
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // A zero flag set deliberately omits MOVEFILE_REPLACE_EXISTING.
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) };
    if result != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
)))]
fn rename_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unsupported on this platform",
    ))
}

pub fn delete(path: &str, recursive: bool) -> Result<FileMutationV1, FileControlError> {
    let path = normalized(path)?;
    reject_link_or_reparse_components(&path, true)?;
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

fn reject_link_or_reparse_components(
    path: &Path,
    reject_target: bool,
) -> Result<(), FileControlError> {
    let first = if reject_target {
        Some(path)
    } else {
        path.parent()
    };
    for component_path in first.into_iter().flat_map(Path::ancestors) {
        let metadata = fs::symlink_metadata(component_path).map_err(map_error)?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(FileControlError::InvalidPath);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(any(unix, windows)))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
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
            [path] => list(path).and_then(serialize_file_list),
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

fn serialize_file_list(value: FileListV1) -> Result<Vec<u8>, FileControlError> {
    // Entry count and platform path/name limits bound the typed allocation.
    // Serialize once, then refuse transport output above the shared hard cap.
    let serialized = serialize(value)?;
    if serialized.len() > MAX_FILE_LIST_BYTES {
        return Err(FileControlError::ResultTooLarge);
    }
    Ok(serialized)
}

fn invalid_arguments(task: &Task, usage: &str) -> TaskResult {
    failure(task, "invalid_arguments", usage)
}

#[cfg(test)]
mod tests {
    use super::serialize_file_list;
    use nw_profile::control::{
        FILE_LIST_SCHEMA_V1, FileControlError, FileEntry, FileListV1, MAX_FILE_LIST_BYTES,
    };

    fn fixture(owner: String) -> FileListV1 {
        FileListV1 {
            schema: FILE_LIST_SCHEMA_V1.to_owned(),
            captured_at: "2026-09-10T12:35:00.000Z".to_owned(),
            path: "/fixture".to_owned(),
            entries: vec![FileEntry {
                name: "entry".to_owned(),
                path: "/fixture/entry".to_owned(),
                kind: "file".to_owned(),
                size: 0,
                modified_at: None,
                permissions: None,
                owner: Some(owner),
            }],
        }
    }

    #[test]
    fn filesystem_list_serialization_accepts_exact_byte_cap_and_rejects_one_more() {
        let base = serde_json::to_vec(&fixture(String::new())).unwrap().len();
        let at_limit = fixture("x".repeat(MAX_FILE_LIST_BYTES - base));
        let serialized = serialize_file_list(at_limit).expect("exact cap is accepted");
        assert_eq!(serialized.len(), MAX_FILE_LIST_BYTES);

        let over_limit = fixture("x".repeat(MAX_FILE_LIST_BYTES - base + 1));
        assert_eq!(
            serialize_file_list(over_limit).unwrap_err(),
            FileControlError::ResultTooLarge
        );
    }
}
