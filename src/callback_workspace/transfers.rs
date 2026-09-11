use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use nw_profile::msgs::{FileAck, FileChunk};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AppError,
    db::{models::FileTransfer, repositories::Repository},
};

pub const DEFAULT_MAX_TRANSFER_BYTES: u64 = 268_435_456;
pub const DEFAULT_TRANSFER_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const CHUNK_BYTES: u64 = 1024;

/// Public transfer status; `storage_key` remains an internal implementation detail.
#[derive(Debug, Clone, Serialize)]
pub struct TransferView {
    pub id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub direction: String,
    pub remote_path: String,
    pub expected_size: Option<i64>,
    pub received_bytes: i64,
    pub sha256: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

impl From<FileTransfer> for TransferView {
    fn from(value: FileTransfer) -> Self {
        Self {
            id: value.id,
            session_id: value.session_id,
            task_id: value.task_id,
            direction: value.direction,
            remote_path: value.remote_path,
            expected_size: value.expected_size,
            received_bytes: value.received_bytes,
            sha256: value.sha256,
            status: value.status,
            error: value.error,
            created_at: value.created_at,
            updated_at: value.updated_at,
            completed_at: value.completed_at,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("transfer protocol id is required")]
    MissingProtocolId,
    #[error("transfer protocol id does not match an authorized transfer")]
    ProtocolMismatch,
    #[error("transfer task id does not match the authorized task")]
    TaskMismatch,
    #[error("transfer is queued behind another transfer")]
    NotActive,
    #[error("transfer offset gap: expected {expected}, got {actual}")]
    OffsetGap { expected: u64, actual: u64 },
    #[error("duplicate transfer bytes do not match persisted bytes")]
    DuplicateMismatch,
    #[error("transfer exceeds the configured {max} byte limit")]
    SizeLimit { max: u64 },
    #[error("transfer size does not match its authorized total")]
    TotalMismatch,
    #[error("transfer checksum mismatch")]
    ChecksumMismatch,
    #[error("transfer artifact is not completed and verified")]
    NotDownloadable,
    #[error("unsafe transfer storage")]
    UnsafeStorage,
    #[error("transfer storage failed")]
    Storage,
    #[error("transfer repository failed")]
    Repository,
}

impl From<AppError> for TransferError {
    fn from(_: AppError) -> Self {
        Self::Repository
    }
}

#[derive(Clone)]
pub struct TransferStore {
    repository: Repository,
    root: PathBuf,
    root_dir: Arc<RootHandle>,
    max_bytes: u64,
    retention_secs: u64,
    locks: Arc<std::sync::Mutex<std::collections::HashMap<String, Weak<tokio::sync::Mutex<()>>>>>,
}

pub struct UploadStage {
    file: Option<File>,
    root_dir: Arc<RootHandle>,
    storage_key: String,
    #[allow(dead_code)]
    operation_lock: TransferOperationLock,
    bytes: u64,
    max_bytes: u64,
    hasher: Sha256,
}

struct RootHandle {
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
    #[cfg(windows)]
    #[allow(dead_code)]
    guard: WindowsPathGuard,
}

/// An OS-backed per-artifact lock. Unlike the SQLite lease, this remains
/// exclusive even when another server process has observed an expired lease.
struct TransferOperationLock {
    #[allow(dead_code)]
    file: File,
}

impl TransferOperationLock {
    fn try_acquire(root: &RootHandle, storage_key: &str) -> Result<Self, TransferError> {
        let name = format!("{storage_key}.lock");
        validate_component(&name)?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd, FromRawFd};

            let name = CString::new(name).map_err(|_| TransferError::UnsafeStorage)?;
            let fd = unsafe {
                libc::openat(
                    root.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(TransferError::Storage);
            }
            let file = unsafe { File::from_raw_fd(fd) };
            if !file
                .metadata()
                .map_err(|_| TransferError::Storage)?
                .file_type()
                .is_file()
            {
                return Err(TransferError::UnsafeStorage);
            }
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
                    tracing::debug!(
                        storage_key,
                        phase = "operation_lock_contended",
                        "transfer storage lock busy"
                    );
                }
                return Err(TransferError::Storage);
            }
            return Ok(Self { file });
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
            };
            use windows_sys::Win32::System::IO::OVERLAPPED;

            let lock_path = root_path(root, &name)?;
            let _guard = WindowsPathGuard::open(&lock_path)?;
            let file = windows_open_path_unchecked_with_sharing(
                &lock_path, true, false, true, false, false, false,
            )?;
            let mut overlapped = OVERLAPPED::default();
            if unsafe {
                LockFileEx(
                    file.as_raw_handle() as _,
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    u32::MAX,
                    u32::MAX,
                    &mut overlapped,
                )
            } == 0
            {
                if std::io::Error::last_os_error().raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION as i32)
                {
                    tracing::debug!(
                        storage_key,
                        phase = "operation_lock_contended",
                        "transfer storage lock busy"
                    );
                }
                return Err(TransferError::Storage);
            }
            return Ok(Self { file });
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(root_path(root, &name)?)
                .map_err(|_| TransferError::Storage)?;
            Ok(Self { file })
        }
    }
}

async fn acquire_operation_lock(
    root: &RootHandle,
    storage_key: &str,
) -> Result<TransferOperationLock, TransferError> {
    const ATTEMPTS: usize = 100;
    for attempt in 0..ATTEMPTS {
        match TransferOperationLock::try_acquire(root, storage_key) {
            Ok(lock) => {
                tracing::debug!(
                    storage_key,
                    phase = "operation_lock_acquired",
                    "transfer storage lock acquired"
                );
                return Ok(lock);
            }
            Err(TransferError::Storage) if attempt + 1 < ATTEMPTS => {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(TransferError::Storage)
}

#[cfg(windows)]
impl Drop for TransferOperationLock {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
        use windows_sys::Win32::System::IO::OVERLAPPED;

        let mut overlapped = OVERLAPPED::default();
        unsafe {
            UnlockFileEx(
                self.file.as_raw_handle() as _,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        };
    }
}

impl UploadStage {
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), TransferError> {
        let next = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or(TransferError::SizeLimit {
                max: self.max_bytes,
            })?;
        if next > self.max_bytes {
            return Err(TransferError::SizeLimit {
                max: self.max_bytes,
            });
        }
        self.file
            .as_mut()
            .ok_or(TransferError::Storage)?
            .write_all(bytes)
            .map_err(|_| TransferError::Storage)?;
        self.hasher.update(bytes);
        self.bytes = next;
        Ok(())
    }
}

impl Drop for UploadStage {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(file) = self.file.take() {
            let _ = windows_delete_open_file(&file);
            return;
        }
        #[cfg(not(windows))]
        if self.file.is_some() {
            let _ = unlink_relative(&self.root_dir, &format!("{}.part", self.storage_key));
        }
    }
}

impl TransferStore {
    pub fn from_config(
        repository: Repository,
        config: &crate::config::Config,
    ) -> Result<Self, TransferError> {
        Self::new_with_retention(
            repository,
            &config.evidence_dir,
            config.max_transfer_bytes,
            config.transfer_retention_secs,
        )
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
    pub fn new(
        repository: Repository,
        evidence_root: impl AsRef<Path>,
        max_bytes: u64,
    ) -> Result<Self, TransferError> {
        Self::new_with_retention(
            repository,
            evidence_root,
            max_bytes,
            DEFAULT_TRANSFER_RETENTION_SECS,
        )
    }

    pub fn new_with_retention(
        repository: Repository,
        evidence_root: impl AsRef<Path>,
        max_bytes: u64,
        retention_secs: u64,
    ) -> Result<Self, TransferError> {
        let evidence_root = evidence_root.as_ref();
        if evidence_root.exists()
            && !std::fs::symlink_metadata(evidence_root)
                .map_err(|_| TransferError::UnsafeStorage)?
                .file_type()
                .is_dir()
        {
            return Err(TransferError::UnsafeStorage);
        }
        let root = evidence_root.join("callback-transfers");
        if root.exists()
            && std::fs::symlink_metadata(&root)
                .map_err(|_| TransferError::UnsafeStorage)?
                .file_type()
                .is_symlink()
        {
            return Err(TransferError::UnsafeStorage);
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&root).map_err(|_| TransferError::Storage)?;
        #[cfg(unix)]
        let held_root = open_or_create_directory(&root)?;
        let root = root
            .canonicalize()
            .map_err(|_| TransferError::UnsafeStorage)?;
        if !root.is_dir() || max_bytes == 0 || max_bytes > i64::MAX as u64 {
            return Err(TransferError::UnsafeStorage);
        }
        #[cfg(unix)]
        let root_dir = Arc::new(held_root);
        #[cfg(not(unix))]
        let root_dir = Arc::new(open_directory(&root)?);
        Ok(Self {
            repository,
            root,
            root_dir,
            max_bytes,
            retention_secs,
            locks: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        })
    }

    pub fn configured(repository: Repository) -> Result<Self, TransferError> {
        let evidence_root = std::env::var("NAUGHTYWOLF_EVIDENCE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("evidence"));
        let max_bytes = std::env::var("NAUGHTYWOLF_MAX_TRANSFER_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_TRANSFER_BYTES);
        let retention_secs = std::env::var("NAUGHTYWOLF_TRANSFER_RETENTION_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TRANSFER_RETENTION_SECS);
        Self::new_with_retention(repository, evidence_root, max_bytes, retention_secs)
    }

    pub async fn queue_download(
        &self,
        session_id: &str,
        remote_path: &str,
        expected_size: Option<u64>,
        expected_sha256: Option<&str>,
        operator_id: &str,
        operator_name: &str,
    ) -> Result<FileTransfer, TransferError> {
        if expected_size.is_some_and(|size| size > self.max_bytes) {
            return Err(TransferError::SizeLimit {
                max: self.max_bytes,
            });
        }
        if expected_sha256.is_some_and(|value| !valid_sha256(value)) {
            return Err(TransferError::ChecksumMismatch);
        }
        let transfer_id = Uuid::new_v4().to_string();
        let storage_key = Uuid::new_v4().simple().to_string();
        self.repository
            .enqueue_file_transfer_with_audit(
                &transfer_id,
                session_id,
                "download",
                remote_path,
                &storage_key,
                expected_size,
                expected_sha256,
                operator_id,
                operator_name,
            )
            .await
            .map_err(Into::into)
    }

    pub fn begin_upload_stage(&self) -> Result<UploadStage, TransferError> {
        let storage_key = Uuid::new_v4().simple().to_string();
        self.safe_path(&format!("{storage_key}.part"))?;
        let operation_lock = TransferOperationLock::try_acquire(&self.root_dir, &storage_key)?;
        let file = create_new_relative(&self.root_dir, &format!("{storage_key}.part"))?;
        sync_directory_handle(&self.root_dir)?;
        Ok(UploadStage {
            file: Some(file),
            root_dir: self.root_dir.clone(),
            storage_key,
            operation_lock,
            bytes: 0,
            max_bytes: self.max_bytes,
            hasher: Sha256::new(),
        })
    }

    pub async fn finish_upload_stage(
        &self,
        mut stage: UploadStage,
        session_id: &str,
        remote_path: &str,
        operator_id: &str,
        operator_name: &str,
    ) -> Result<FileTransfer, TransferError> {
        let file = stage.file.take().ok_or(TransferError::Storage)?;
        file.sync_all().map_err(|_| TransferError::Storage)?;
        #[cfg(not(windows))]
        drop(file);
        publish_no_replace_at(
            &stage.root_dir,
            &format!("{}.part", stage.storage_key),
            &stage.storage_key,
            #[cfg(windows)]
            &file,
        )?;
        sync_directory_handle(&stage.root_dir)?;
        let sha256 = hex::encode(stage.hasher.clone().finalize());
        let transfer_id = Uuid::new_v4().to_string();
        self.repository
            .enqueue_file_transfer_with_audit(
                &transfer_id,
                session_id,
                "upload",
                remote_path,
                &stage.storage_key,
                Some(stage.bytes),
                Some(&sha256),
                operator_id,
                operator_name,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn transfer(&self, transfer_id: &str) -> Result<Option<FileTransfer>, TransferError> {
        self.repository
            .file_transfer(transfer_id)
            .await
            .map_err(Into::into)
    }

    /// Remove staged/orphaned artifacts which are no longer owned by a live
    /// queued or active transfer. This is safe to run at every server start.
    pub async fn cleanup_orphans(&self) -> Result<(), TransferError> {
        let transfers = self.repository.list_all_file_transfers().await?;
        self.cleanup_expired(&transfers).await?;
        let transfers = self.repository.list_all_file_transfers().await?;
        let active_ids: std::collections::HashSet<_> = transfers
            .iter()
            .filter(|transfer| matches!(transfer.status.as_str(), "queued" | "active"))
            .map(|transfer| transfer.id.clone())
            .collect();
        self.locks
            .lock()
            .expect("transfer lock map poisoned")
            .retain(|id, lock| active_ids.contains(id) && lock.upgrade().is_some());
        let live: std::collections::HashSet<_> = transfers
            .iter()
            .filter(|transfer| matches!(transfer.status.as_str(), "queued" | "active"))
            .flat_map(|transfer| {
                [
                    transfer.storage_key.clone(),
                    format!("{}.part", transfer.storage_key),
                    format!("{}.lock", transfer.storage_key),
                ]
            })
            .collect();
        let retained_completed: std::collections::HashSet<_> = transfers
            .iter()
            .filter(|transfer| transfer.status == "completed")
            .flat_map(|transfer| {
                [
                    transfer.storage_key.clone(),
                    format!("{}.lock", transfer.storage_key),
                ]
            })
            .collect();
        for name in read_directory_names(&self.root_dir)? {
            if name.ends_with(".lock") {
                continue;
            }
            if !live.contains(&name) && !retained_completed.contains(&name) {
                let operation_lock = match generated_storage_key(&name) {
                    Some(storage_key) => {
                        Some(acquire_operation_lock(&self.root_dir, storage_key).await?)
                    }
                    None => None,
                };
                if let Some(storage_key) = generated_storage_key(&name) {
                    let still_owned =
                        self.repository
                            .list_all_file_transfers()
                            .await?
                            .iter()
                            .any(|transfer| {
                                transfer.storage_key == storage_key
                                    && matches!(
                                        transfer.status.as_str(),
                                        "queued" | "active" | "completed"
                                    )
                            });
                    if still_owned {
                        drop(operation_lock);
                        continue;
                    }
                }
                if let Ok(file) = open_existing_regular_at(&self.root_dir, &name, false) {
                    drop(file);
                    unlink_relative(&self.root_dir, &name)?;
                }
                drop(operation_lock);
            }
        }
        sync_directory_handle(&self.root_dir)
    }

    async fn cleanup_expired(&self, transfers: &[FileTransfer]) -> Result<(), TransferError> {
        let now = chrono::Utc::now();
        for transfer in transfers {
            if !matches!(
                transfer.status.as_str(),
                "completed" | "error" | "cancelled"
            ) {
                continue;
            }
            let stamp = transfer
                .completed_at
                .as_deref()
                .unwrap_or(&transfer.updated_at);
            let expired = chrono::DateTime::parse_from_rfc3339(stamp)
                .ok()
                .map(|value| {
                    let age = now
                        .signed_duration_since(value.with_timezone(&chrono::Utc))
                        .num_seconds();
                    age >= 0 && (age as u64) >= self.retention_secs
                })
                .unwrap_or(false);
            if !expired {
                continue;
            }
            {
                let _operation_lock =
                    acquire_operation_lock(&self.root_dir, &transfer.storage_key).await?;
                let lease = self.acquire_lease(&transfer.id).await?;
                for name in [
                    transfer.storage_key.clone(),
                    format!("{}.part", transfer.storage_key),
                ] {
                    self.renew_lease(&transfer.id, &lease).await?;
                    if let Ok(file) = open_existing_regular_at(&self.root_dir, &name, false) {
                        drop(file);
                        unlink_relative(&self.root_dir, &name)?;
                    }
                }
                let deleted = sqlx::query("DELETE FROM c2_file_transfers WHERE id = ? AND status IN ('completed','error','cancelled')")
                    .bind(&transfer.id)
                    .execute(&self.repository.pool)
                    .await
                    .map_err(|_| TransferError::Repository)?;
                if deleted.rows_affected() != 1 {
                    self.release_lease(&transfer.id, &lease).await?;
                    return Err(TransferError::Repository);
                }
                self.release_lease(&transfer.id, &lease).await?;
            }
            unlink_relative(&self.root_dir, &format!("{}.lock", transfer.storage_key))?;
        }
        Ok(())
    }

    pub async fn receive_chunk(
        &self,
        session_id: &str,
        chunk: &FileChunk,
    ) -> Result<FileAck, TransferError> {
        let transfer_id = chunk.transfer_id.ok_or(TransferError::MissingProtocolId)?;
        let lock = self.lock_for(&transfer_id.to_string());
        let _guard = lock.lock().await;
        let transfer = self
            .repository
            .file_transfer_for_session(session_id, &transfer_id.to_string())
            .await?
            .ok_or(TransferError::ProtocolMismatch)?;
        let _operation_lock = acquire_operation_lock(&self.root_dir, &transfer.storage_key).await?;
        let lease = self.acquire_lease(&transfer_id.to_string()).await?;
        let result = self
            .receive_chunk_inner(session_id, chunk, transfer_id, &lease)
            .await;
        self.release_lease(&transfer_id.to_string(), &lease).await?;
        tracing::debug!(%transfer_id, phase = "operation_complete", "transfer storage operation complete");
        result
    }

    async fn receive_chunk_inner(
        &self,
        session_id: &str,
        chunk: &FileChunk,
        transfer_id: Uuid,
        lease: &str,
    ) -> Result<FileAck, TransferError> {
        let task_id = chunk.task_id.ok_or(TransferError::MissingProtocolId)?;
        let Some(mut transfer) = self
            .repository
            .file_transfer_for_session(session_id, &transfer_id.to_string())
            .await?
        else {
            return Err(TransferError::ProtocolMismatch);
        };
        if transfer.direction != "download"
            || transfer.task_id.as_deref() != Some(&task_id.to_string())
        {
            return Err(TransferError::TaskMismatch);
        }
        if transfer.status == "completed" {
            let total = transfer
                .expected_size
                .and_then(|value| u64::try_from(value).ok())
                .ok_or(TransferError::TotalMismatch)?;
            let end = chunk
                .offset
                .checked_add(chunk.data.len() as u64)
                .ok_or(TransferError::TotalMismatch)?;
            if chunk.total != total || end > total {
                return Err(TransferError::TotalMismatch);
            }
            let mut file = open_existing_regular_at(&self.root_dir, &transfer.storage_key, false)?;
            file.seek(SeekFrom::Start(chunk.offset))
                .map_err(|_| TransferError::Storage)?;
            let mut persisted = vec![0; chunk.data.len()];
            file.read_exact(&mut persisted)
                .map_err(|_| TransferError::Storage)?;
            if persisted != chunk.data {
                return Err(TransferError::DuplicateMismatch);
            }
            return Ok(FileAck {
                transfer_id: Some(transfer_id),
                received: total,
                total,
                done: true,
            });
        }
        self.activate_if_next(&mut transfer, "download").await?;
        if transfer.status != "active" {
            return Err(TransferError::NotActive);
        }
        let chunk_end = chunk
            .offset
            .checked_add(chunk.data.len() as u64)
            .ok_or(TransferError::TotalMismatch)?;
        if chunk_end > chunk.total {
            self.fail(&transfer.id, "transfer chunk exceeds declared total")
                .await?;
            return Err(TransferError::TotalMismatch);
        }
        if chunk.total > self.max_bytes || chunk_end > self.max_bytes {
            self.fail(&transfer.id, "transfer exceeds configured size limit")
                .await?;
            return Err(TransferError::SizeLimit {
                max: self.max_bytes,
            });
        }
        if transfer
            .expected_size
            .is_some_and(|size| size < 0 || size as u64 != chunk.total)
        {
            self.fail(
                &transfer.id,
                "transfer total does not match authorized size",
            )
            .await?;
            return Err(TransferError::TotalMismatch);
        }
        if transfer.expected_size.is_none() {
            sqlx::query("UPDATE c2_file_transfers SET expected_size = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?")
                .bind(chunk.total as i64)
                .bind(&transfer.id)
                .execute(&self.repository.pool)
                .await
                .map_err(|_| TransferError::Repository)?;
            transfer.expected_size = Some(chunk.total as i64);
        }
        let received =
            u64::try_from(transfer.received_bytes).map_err(|_| TransferError::Repository)?;
        let part_name = format!("{}.part", transfer.storage_key);
        let completed_name = transfer.storage_key.clone();
        if reconcile_completed_download(
            &self.repository,
            &mut transfer,
            &self.root_dir,
            &completed_name,
            chunk.total,
            self.max_bytes,
            lease,
        )
        .await?
        {
            return Ok(FileAck {
                transfer_id: Some(transfer_id),
                received: chunk.total,
                total: chunk.total,
                done: true,
            });
        }
        if chunk.offset > received {
            return Err(TransferError::OffsetGap {
                expected: received,
                actual: chunk.offset,
            });
        }
        if chunk.offset < received {
            let end = chunk
                .offset
                .checked_add(chunk.data.len() as u64)
                .ok_or(TransferError::TotalMismatch)?;
            if end > received {
                return Err(TransferError::DuplicateMismatch);
            }
            let mut file = open_existing_regular_at(&self.root_dir, &part_name, true)?;
            file.seek(SeekFrom::Start(chunk.offset))
                .map_err(|_| TransferError::Storage)?;
            let mut persisted = vec![0; chunk.data.len()];
            file.read_exact(&mut persisted)
                .map_err(|_| TransferError::Storage)?;
            if persisted != chunk.data {
                return Err(TransferError::DuplicateMismatch);
            }
            return Ok(FileAck {
                transfer_id: Some(transfer_id),
                received,
                total: chunk.total,
                done: false,
            });
        }

        let mut file = if received == 0 {
            match create_new_relative(&self.root_dir, &part_name) {
                Ok(file) => file,
                Err(TransferError::Storage) => {
                    open_existing_regular_at(&self.root_dir, &part_name, true)?
                }
                Err(error) => return Err(error),
            }
        } else {
            open_existing_regular_at(&self.root_dir, &part_name, true)?
        };
        let metadata = file.metadata().map_err(|_| TransferError::Storage)?;
        if !metadata.file_type().is_file() {
            return Err(TransferError::UnsafeStorage);
        }
        if metadata.len() != received {
            self.fail(&transfer.id, "transfer staging prefix is inconsistent")
                .await?;
            return Err(TransferError::Storage);
        }
        self.renew_lease(&transfer.id, lease).await?;
        file.set_len(received).map_err(|_| TransferError::Storage)?;
        file.seek(SeekFrom::Start(received))
            .map_err(|_| TransferError::Storage)?;
        file.write_all(&chunk.data)
            .map_err(|_| TransferError::Storage)?;
        tracing::debug!(%transfer_id, phase = "file_sync", "synchronizing transfer file");
        file.sync_all().map_err(|_| TransferError::Storage)?;
        self.renew_lease(&transfer.id, lease).await?;
        tracing::debug!(%transfer_id, phase = "part_directory_sync", "synchronizing staging directory");
        sync_directory_handle(&self.root_dir)?;
        let next = received
            .checked_add(chunk.data.len() as u64)
            .ok_or(TransferError::TotalMismatch)?;
        let progress = sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND received_bytes = ?")
            .bind(next as i64)
            .bind(&transfer.id)
            .bind(received as i64)
            .execute(&self.repository.pool)
            .await
            .map_err(|_| TransferError::Repository)?;
        if progress.rows_affected() != 1 {
            return Err(TransferError::Repository);
        }

        if next == chunk.total {
            self.renew_lease(&transfer.id, lease).await?;
            #[cfg(windows)]
            let mut verify = file.try_clone().map_err(|_| TransferError::Storage)?;
            #[cfg(not(windows))]
            let mut verify = open_existing_regular_at(&self.root_dir, &part_name, false)?;
            tracing::debug!(%transfer_id, phase = "hash", "verifying transfer digest");
            let digest =
                sha256_open_file_with_lease(&self.repository, &mut verify, &transfer.id, lease)
                    .await?;
            if transfer
                .sha256
                .as_deref()
                .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest))
            {
                self.fail(&transfer.id, "SHA-256 mismatch").await?;
                return Err(TransferError::ChecksumMismatch);
            }
            tracing::debug!(%transfer_id, phase = "publish", "publishing verified transfer");
            publish_no_replace_at(
                &self.root_dir,
                &part_name,
                &transfer.storage_key,
                #[cfg(windows)]
                &file,
            )?;
            tracing::debug!(%transfer_id, phase = "publication_directory_sync", "synchronizing published directory");
            sync_directory_handle(&self.root_dir)?;
            tracing::debug!(%transfer_id, phase = "durable_update", "persisting verified transfer state");
            let verified = sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, sha256 = ?, error = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND status = 'active'")
                .bind(next as i64)
                .bind(&digest)
                .bind(&transfer.id)
                .execute(&self.repository.pool)
                .await
                .map_err(|_| TransferError::Repository)?;
            if verified.rows_affected() != 1 {
                return Err(TransferError::Repository);
            }
            tracing::debug!(%transfer_id, phase = "durable_update_complete", "verified transfer state persisted");
            transfer.received_bytes = next as i64;
            transfer.sha256 = Some(digest.clone());
            return Ok(FileAck {
                transfer_id: Some(transfer_id),
                received: next,
                total: chunk.total,
                done: true,
            });
        }
        Ok(FileAck {
            transfer_id: Some(transfer_id),
            received: next,
            total: chunk.total,
            done: false,
        })
    }

    pub async fn read_completed(&self, transfer: &FileTransfer) -> Result<Vec<u8>, TransferError> {
        if transfer.direction != "download"
            || transfer.status != "completed"
            || transfer.sha256.is_none()
        {
            return Err(TransferError::NotDownloadable);
        }
        let mut file = open_existing_regular_at(&self.root_dir, &transfer.storage_key, false)?;
        let metadata = file.metadata().map_err(|_| TransferError::Storage)?;
        let expected_size = transfer
            .expected_size
            .and_then(|size| u64::try_from(size).ok())
            .ok_or(TransferError::NotDownloadable)?;
        if metadata.len() != expected_size || metadata.len() > self.max_bytes {
            return Err(TransferError::ChecksumMismatch);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| TransferError::Storage)?;
        if bytes.len() as u64 > self.max_bytes
            || !transfer.sha256.as_deref().is_some_and(|expected| {
                expected.eq_ignore_ascii_case(&hex::encode(Sha256::digest(&bytes)))
            })
        {
            return Err(TransferError::ChecksumMismatch);
        }
        Ok(bytes)
    }

    /// Verify the completed artifact and return its bounded, read-only file handle.
    pub fn open_completed(&self, transfer: &FileTransfer) -> Result<File, TransferError> {
        if transfer.direction != "download"
            || transfer.status != "completed"
            || transfer.sha256.is_none()
        {
            return Err(TransferError::NotDownloadable);
        }
        let mut file = open_existing_regular_at(&self.root_dir, &transfer.storage_key, false)?;
        let metadata = file.metadata().map_err(|_| TransferError::Storage)?;
        let expected_size = transfer
            .expected_size
            .and_then(|size| u64::try_from(size).ok())
            .ok_or(TransferError::NotDownloadable)?;
        if !metadata.file_type().is_file() || metadata.len() != expected_size {
            return Err(TransferError::ChecksumMismatch);
        }
        let digest = sha256_open_file(&mut file)?;
        if !transfer
            .sha256
            .as_deref()
            .is_some_and(|expected| expected.eq_ignore_ascii_case(&digest))
        {
            return Err(TransferError::ChecksumMismatch);
        }
        Ok(file)
    }

    pub async fn next_upload_chunks(
        &self,
        session_id: &str,
        budget: usize,
    ) -> Result<Vec<FileChunk>, TransferError> {
        let Some(transfer) = self
            .repository
            .next_file_transfer(session_id, "upload")
            .await?
        else {
            return Ok(Vec::new());
        };
        let lock = self.lock_for(&transfer.id);
        let _guard = lock.lock().await;
        let lease = self.acquire_lease(&transfer.id).await?;
        let transfer_id = transfer.id.clone();
        let result = self
            .next_upload_chunks_inner(session_id, budget, transfer, &lease)
            .await;
        self.release_lease(&transfer_id, &lease).await?;
        result
    }

    async fn next_upload_chunks_inner(
        &self,
        _session_id: &str,
        budget: usize,
        mut transfer: FileTransfer,
        lease: &str,
    ) -> Result<Vec<FileChunk>, TransferError> {
        self.activate_if_next(&mut transfer, "upload").await?;
        if transfer.status != "active" || budget == 0 {
            return Ok(Vec::new());
        }
        let task_id = transfer
            .task_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or(TransferError::TaskMismatch)?;
        let transfer_id =
            Uuid::parse_str(&transfer.id).map_err(|_| TransferError::ProtocolMismatch)?;
        let total = transfer
            .expected_size
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(TransferError::TotalMismatch)?;
        if total == 0 {
            return Ok(vec![FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: transfer.remote_path.clone(),
                offset: 0,
                total: 0,
                data: Vec::new(),
            }]);
        }
        let offset =
            u64::try_from(transfer.received_bytes).map_err(|_| TransferError::Repository)?;
        let mut file = open_existing_regular_at(&self.root_dir, &transfer.storage_key, false)?;
        self.renew_lease(&transfer.id, lease).await?;
        let mut chunks = Vec::new();
        let mut cursor = offset;
        let mut remaining = budget as u64;
        while cursor < total && remaining > 0 {
            let take = CHUNK_BYTES.min(remaining).min(total - cursor) as usize;
            file.seek(SeekFrom::Start(cursor))
                .map_err(|_| TransferError::Storage)?;
            let mut data = vec![0; take];
            file.read_exact(&mut data)
                .map_err(|_| TransferError::Storage)?;
            chunks.push(FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: transfer.remote_path.clone(),
                offset: cursor,
                total,
                data,
            });
            cursor += take as u64;
            remaining -= take as u64;
        }
        Ok(chunks)
    }

    pub async fn ack_upload(
        &self,
        session_id: &str,
        ack: &FileAck,
    ) -> Result<FileAck, TransferError> {
        let id = ack.transfer_id.ok_or(TransferError::MissingProtocolId)?;
        let lock = self.lock_for(&id.to_string());
        let _guard = lock.lock().await;
        let lease = self.acquire_lease(&id.to_string()).await?;
        let result = self.ack_upload_inner(session_id, ack, id).await;
        self.release_lease(&id.to_string(), &lease).await?;
        result
    }

    async fn ack_upload_inner(
        &self,
        session_id: &str,
        ack: &FileAck,
        id: Uuid,
    ) -> Result<FileAck, TransferError> {
        let Some(transfer) = self
            .repository
            .file_transfer_for_session(session_id, &id.to_string())
            .await?
        else {
            return Err(TransferError::ProtocolMismatch);
        };
        if transfer.direction != "upload" {
            return Err(TransferError::NotActive);
        }
        let total = transfer
            .expected_size
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(TransferError::TotalMismatch)?;
        if (ack.total != 0 && ack.total != total)
            || ack.received > total
            || (ack.done && ack.received != total)
        {
            return Err(TransferError::TotalMismatch);
        }
        let current =
            u64::try_from(transfer.received_bytes).map_err(|_| TransferError::Repository)?;
        if transfer.status == "completed"
            && ack.done
            && ack.received == current
            && ack.received == total
        {
            return Ok(*ack);
        }
        if transfer.status != "active" {
            return Err(TransferError::NotActive);
        }
        if ack.received < current {
            return Ok(*ack);
        }
        // A terminal receiver ACK only proves that the bytes reached its
        // transfer-specific staging file. The implant publishes and confirms
        // the destination via its task result, so keep this provisional.
        let status = "active";
        let update = sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, status = ?, completed_at = CASE WHEN ? THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE completed_at END, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND received_bytes = ? AND status = 'active'")
            .bind(ack.received as i64).bind(status).bind(ack.done).bind(&transfer.id)
            .bind(current as i64)
            .execute(&self.repository.pool).await.map_err(|_| TransferError::Repository)?;
        if update.rows_affected() != 1 {
            return Err(TransferError::Repository);
        }
        Ok(*ack)
    }

    async fn activate_if_next(
        &self,
        transfer: &mut FileTransfer,
        direction: &str,
    ) -> Result<(), TransferError> {
        if transfer.status != "queued" {
            return Ok(());
        }
        let update = sqlx::query("UPDATE c2_file_transfers SET status = 'active', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND status = 'queued' AND NOT EXISTS (SELECT 1 FROM c2_file_transfers predecessor WHERE predecessor.session_id = c2_file_transfers.session_id AND predecessor.direction = ? AND predecessor.status IN ('queued', 'active') AND (predecessor.created_at < c2_file_transfers.created_at OR (predecessor.created_at = c2_file_transfers.created_at AND predecessor.id < c2_file_transfers.id)))")
            .bind(&transfer.id).bind(direction).execute(&self.repository.pool).await.map_err(|_| TransferError::Repository)?;
        if update.rows_affected() != 1 {
            return Ok(());
        }
        transfer.status = "active".to_owned();
        Ok(())
    }

    async fn fail(&self, transfer_id: &str, error: &str) -> Result<(), TransferError> {
        let _update = sqlx::query("UPDATE c2_file_transfers SET status = 'error', error = ?, completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND status IN ('queued', 'active')")
            .bind(error).bind(transfer_id).execute(&self.repository.pool).await.map_err(|_| TransferError::Repository)?;
        Ok(())
    }

    fn safe_path(&self, file_name: &str) -> Result<PathBuf, TransferError> {
        validate_component(file_name)?;
        // This path is retained for UI/tests only. Actual file operations use
        // the directory handle held at construction, so replacing an ancestor
        // cannot redirect reads, writes, or publication.
        Ok(self.root.join(file_name))
    }

    fn lock_for(&self, transfer_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().expect("transfer lock map poisoned");
        if let Some(lock) = locks.get(transfer_id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(transfer_id.to_owned(), Arc::downgrade(&lock));
        lock
    }

    async fn acquire_lease(&self, transfer_id: &str) -> Result<String, TransferError> {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM c2_file_transfers WHERE id = ?")
                .bind(transfer_id)
                .fetch_optional(&self.repository.pool)
                .await
                .map_err(|_| TransferError::Repository)?;
        if exists.is_none() {
            return Err(TransferError::ProtocolMismatch);
        }
        let owner = Uuid::new_v4().to_string();
        for _ in 0..200 {
            let mut transaction = self
                .repository
                .pool
                .begin()
                .await
                .map_err(|_| TransferError::Repository)?;
            sqlx::query("DELETE FROM c2_transfer_leases WHERE expires_at <= unixepoch()")
                .execute(&mut *transaction)
                .await
                .map_err(|_| TransferError::Repository)?;
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO c2_transfer_leases (transfer_id, owner, expires_at) VALUES (?, ?, unixepoch() + 30)",
            )
            .bind(transfer_id)
            .bind(&owner)
            .execute(&mut *transaction)
            .await
            .map_err(|_| TransferError::Repository)?;
            transaction
                .commit()
                .await
                .map_err(|_| TransferError::Repository)?;
            if inserted.rows_affected() == 1 {
                return Ok(owner);
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        Err(TransferError::Repository)
    }

    async fn release_lease(&self, transfer_id: &str, owner: &str) -> Result<(), TransferError> {
        sqlx::query("DELETE FROM c2_transfer_leases WHERE transfer_id = ? AND owner = ?")
            .bind(transfer_id)
            .bind(owner)
            .execute(&self.repository.pool)
            .await
            .map_err(|_| TransferError::Repository)?;
        Ok(())
    }

    async fn renew_lease(&self, transfer_id: &str, owner: &str) -> Result<(), TransferError> {
        let _update = sqlx::query(
            "UPDATE c2_transfer_leases SET expires_at = unixepoch() + 30 WHERE transfer_id = ? AND owner = ? AND expires_at > unixepoch()",
        )
        .bind(transfer_id)
        .bind(owner)
        .execute(&self.repository.pool)
        .await
        .map_err(|_| TransferError::Repository)?;
        Ok(())
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn reconcile_completed_download(
    repository: &Repository,
    transfer: &mut FileTransfer,
    root_dir: &RootHandle,
    completed_name: &str,
    total: u64,
    max_bytes: u64,
    lease: &str,
) -> Result<bool, TransferError> {
    let mut completed = match open_existing_regular_at(root_dir, completed_name, false) {
        Ok(file) => file,
        Err(TransferError::Storage) => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = completed.metadata().map_err(|_| TransferError::Storage)?;
    if !metadata.file_type().is_file() || metadata.len() != total || total > max_bytes {
        return Err(TransferError::UnsafeStorage);
    }
    let digest =
        sha256_open_file_with_lease(repository, &mut completed, &transfer.id, lease).await?;
    if transfer
        .sha256
        .as_deref()
        .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest))
    {
        return Err(TransferError::ChecksumMismatch);
    }
    let update = sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, sha256 = ?, error = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND status = 'active'")
        .bind(total as i64)
        .bind(&digest)
        .bind(&transfer.id)
        .execute(&repository.pool)
        .await
        .map_err(|_| TransferError::Repository)?;
    if update.rows_affected() != 1 {
        return Err(TransferError::Repository);
    }
    transfer.received_bytes = total as i64;
    transfer.sha256 = Some(digest);
    // Artifact publication is provisional until the receiver reports that it
    // successfully published the destination path.
    transfer.status = "active".to_owned();
    Ok(true)
}

fn publish_no_replace_at(
    root: &RootHandle,
    part: &str,
    completed: &str,
    #[cfg(windows)] source: &File,
) -> Result<(), TransferError> {
    validate_component(part)?;
    validate_component(completed)?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let part = CString::new(part).map_err(|_| TransferError::UnsafeStorage)?;
        let completed = CString::new(completed).map_err(|_| TransferError::UnsafeStorage)?;
        let result = unsafe {
            libc::linkat(
                root.file.as_raw_fd(),
                part.as_ptr(),
                root.file.as_raw_fd(),
                completed.as_ptr(),
                0,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
                TransferError::UnsafeStorage
            } else {
                TransferError::Storage
            });
        }
        let result = unsafe { libc::unlinkat(root.file.as_raw_fd(), part.as_ptr(), 0) };
        if result != 0 {
            return Err(TransferError::Storage);
        }
        Ok(())
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let part_path = root_path(root, part)?;
        let completed_path = root_path(root, completed)?;
        std::fs::hard_link(part_path, completed_path).map_err(|_| TransferError::Storage)?;
        std::fs::remove_file(part_path).map_err(|_| TransferError::Storage)
    }
    #[cfg(windows)]
    {
        let part_path = root_path(root, part)?;
        let completed_path = root_path(root, completed)?;
        let _part_guard = WindowsPathGuard::open(&part_path)?;
        let _completed_guard = WindowsPathGuard::open(&completed_path)?;
        let part_w = windows_wide(&part_path);
        let completed_w = windows_wide(&completed_path);
        let linked = unsafe {
            windows_sys::Win32::Storage::FileSystem::CreateHardLinkW(
                completed_w.as_ptr(),
                part_w.as_ptr(),
                std::ptr::null(),
            )
        };
        if linked == 0 {
            return Err(TransferError::Storage);
        }
        windows_delete_open_file(source)
    }
}

#[cfg(unix)]
fn open_or_create_directory(path: &Path) -> Result<RootHandle, TransferError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::OpenOptionsExt;

    let mut absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| TransferError::UnsafeStorage)?
            .join(path)
    };
    // macOS exposes /var as a system symlink to /private/var. Resolve this
    // fixed system alias before the component-by-component no-follow walk;
    // user-controlled ancestors are still opened with O_NOFOLLOW below.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if absolute.starts_with("/var") {
        absolute = PathBuf::from("/private").join(absolute.strip_prefix("/").unwrap());
    }
    let mut components = absolute.components();
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(Path::new("/"))
        .map_err(|_| TransferError::UnsafeStorage)?;
    while let Some(component) = components.next() {
        let name = match component {
            std::path::Component::RootDir | std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => name,
            std::path::Component::ParentDir => return Err(TransferError::UnsafeStorage),
            _ => return Err(TransferError::UnsafeStorage),
        };
        let name = CString::new(name.to_string_lossy().as_bytes())
            .map_err(|_| TransferError::UnsafeStorage)?;
        let fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            )
        };
        let next = if fd >= 0 {
            unsafe { File::from_raw_fd(fd) }
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(TransferError::UnsafeStorage);
            }
            let made = unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) };
            if made != 0 {
                let race = std::io::Error::last_os_error();
                if race.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(TransferError::UnsafeStorage);
                }
            }
            let fd = unsafe {
                libc::openat(
                    current.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0,
                )
            };
            if fd < 0 {
                return Err(TransferError::UnsafeStorage);
            }
            unsafe { File::from_raw_fd(fd) }
        };
        if !next
            .metadata()
            .map_err(|_| TransferError::UnsafeStorage)?
            .file_type()
            .is_dir()
        {
            return Err(TransferError::UnsafeStorage);
        }
        current = next;
    }
    Ok(RootHandle { file: current })
}

#[cfg(not(unix))]
fn open_directory(path: &Path) -> Result<RootHandle, TransferError> {
    #[cfg(not(windows))]
    {
        return Ok(RootHandle {
            file: File::open(path).map_err(|_| TransferError::UnsafeStorage)?,
            path: path.to_path_buf(),
        });
    }
    #[cfg(windows)]
    {
        let guard = WindowsPathGuard::open(path)?;
        let file = windows_open_path_unchecked_with_sharing(
            path, false, false, false, true, false, false,
        )?;
        Ok(RootHandle {
            file,
            path: path.to_path_buf(),
            guard,
        })
    }
}

fn create_new_relative(root: &RootHandle, name: &str) -> Result<File, TransferError> {
    validate_component(name)?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = CString::new(name).map_err(|_| TransferError::UnsafeStorage)?;
        let fd = unsafe {
            libc::openat(
                root.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(TransferError::Storage);
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(|_| TransferError::Storage)?;
        if !metadata.file_type().is_file() {
            return Err(TransferError::UnsafeStorage);
        }
        Ok(file)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(root_path(root, name)?)
            .map_err(|_| TransferError::Storage)
    }
    #[cfg(windows)]
    {
        windows_open_regular_path(&root_path(root, name)?, true, true)
    }
}

fn unlink_relative(root: &RootHandle, name: &str) -> Result<(), TransferError> {
    validate_component(name)?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let name = CString::new(name).map_err(|_| TransferError::UnsafeStorage)?;
        let result = unsafe { libc::unlinkat(root.file.as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(());
            }
            return Err(TransferError::Storage);
        }
        Ok(())
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        match std::fs::remove_file(root_path(root, name)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(TransferError::Storage),
        }
    }
    #[cfg(windows)]
    {
        let path = root_path(root, name)?;
        windows_delete_path(&path)
    }
}

#[cfg(unix)]
fn read_directory_names(root: &RootHandle) -> Result<Vec<String>, TransferError> {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd;

    struct DirectoryStream(*mut libc::DIR);

    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            // fdopendir takes ownership of the duplicated fd, so closedir is
            // the one operation that must release it on every early return.
            unsafe { libc::closedir(self.0) };
        }
    }

    // Open a fresh directory description rather than dup'ing the held root:
    // dup shares the directory offset and can make concurrent enumeration
    // skip entries. No pathname is used, so an ancestor swap cannot redirect
    // cleanup.
    let current = std::ffi::CString::new(".").expect("literal has no NUL");
    let duplicate = unsafe {
        libc::openat(
            root.file.as_raw_fd(),
            current.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if duplicate < 0 {
        return Err(TransferError::Storage);
    }
    let directory = unsafe { libc::fdopendir(duplicate) };
    if directory.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(TransferError::Storage);
    }
    let directory = DirectoryStream(directory);
    let mut names = Vec::new();
    loop {
        set_errno_zero();
        let entry = unsafe { libc::readdir(directory.0) };
        if entry.is_null() {
            if std::io::Error::last_os_error().raw_os_error() == Some(0) {
                break;
            }
            return Err(TransferError::Storage);
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
            .to_str()
            .map_err(|_| TransferError::UnsafeStorage)?;
        if name != "." && name != ".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

#[cfg(unix)]
fn set_errno_zero() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        *libc::__errno_location() = 0;
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    unsafe {
        *libc::__error() = 0;
    }
}

#[cfg(not(unix))]
fn read_directory_names(root: &RootHandle) -> Result<Vec<String>, TransferError> {
    std::fs::read_dir(&root.path)
        .map_err(|_| TransferError::Storage)?
        .map(|entry| {
            entry
                .map_err(|_| TransferError::Storage)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
        })
        .collect()
}

fn open_existing_regular_at(
    root: &RootHandle,
    name: &str,
    write: bool,
) -> Result<File, TransferError> {
    validate_component(name)?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = CString::new(name).map_err(|_| TransferError::UnsafeStorage)?;
        let flags = if write { libc::O_RDWR } else { libc::O_RDONLY } | libc::O_NOFOLLOW;
        let fd = unsafe { libc::openat(root.file.as_raw_fd(), name.as_ptr(), flags, 0) };
        if fd < 0 {
            return Err(TransferError::Storage);
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !file
            .metadata()
            .map_err(|_| TransferError::Storage)?
            .file_type()
            .is_file()
        {
            return Err(TransferError::UnsafeStorage);
        }
        Ok(file)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let mut options = OpenOptions::new();
        options.read(true).write(write);
        options
            .open(root_path(root, name)?)
            .map_err(|_| TransferError::Storage)
    }
    #[cfg(windows)]
    {
        windows_open_regular_path(&root_path(root, name)?, write, false)
    }
}

fn validate_component(name: &str) -> Result<(), TransferError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.as_bytes().contains(&0)
    {
        return Err(TransferError::UnsafeStorage);
    }
    Ok(())
}

fn generated_storage_key(name: &str) -> Option<&str> {
    let storage_key = name.strip_suffix(".part").unwrap_or(name);
    (storage_key.len() == 32 && storage_key.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(storage_key)
}

#[cfg(not(unix))]
fn root_path(root: &RootHandle, name: &str) -> Result<PathBuf, TransferError> {
    validate_component(name)?;
    Ok(root.path.join(name))
}

#[cfg(windows)]
fn windows_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
#[allow(dead_code)]
struct WindowsPathGuard {
    handles: Vec<File>,
    parent: File,
    path: PathBuf,
}

#[cfg(windows)]
impl WindowsPathGuard {
    fn open(path: &Path) -> Result<Self, TransferError> {
        let mut ancestors = Vec::new();
        let mut current = path.parent().ok_or(TransferError::UnsafeStorage)?;
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
            handles.push(windows_open_path_unchecked_with_sharing(
                &ancestor, false, false, false, true, false, false,
            )?);
        }
        let parent = handles.pop().ok_or(TransferError::UnsafeStorage)?;
        Ok(Self {
            handles,
            parent,
            path: path.to_path_buf(),
        })
    }
}

#[cfg(windows)]
fn windows_open_path(
    path: &Path,
    write: bool,
    create_new: bool,
    directory: bool,
) -> Result<File, TransferError> {
    let _guard = WindowsPathGuard::open(path)?;
    windows_open_path_unchecked(path, write, create_new, directory)
}

#[cfg(windows)]
fn windows_open_regular_path(
    path: &Path,
    write: bool,
    create_new: bool,
) -> Result<File, TransferError> {
    let _guard = WindowsPathGuard::open(path)?;
    windows_open_path_unchecked_with_sharing(path, write, create_new, false, false, false, true)
}

#[cfg(windows)]
fn windows_open_path_unchecked(
    path: &Path,
    write: bool,
    create_new: bool,
    directory: bool,
) -> Result<File, TransferError> {
    windows_open_path_unchecked_with_sharing(path, write, create_new, false, directory, true, false)
}

#[cfg(windows)]
fn windows_open_path_unchecked_with_sharing(
    path: &Path,
    write: bool,
    create_new: bool,
    open_always: bool,
    directory: bool,
    share_delete: bool,
    delete_access: bool,
) -> Result<File, TransferError> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_ALWAYS, OPEN_EXISTING,
    };

    let wide = windows_wide(path);
    let mut access = if write {
        GENERIC_READ | GENERIC_WRITE
    } else {
        GENERIC_READ
    };
    if delete_access {
        access |= DELETE;
    }
    let disposition = if create_new {
        CREATE_NEW
    } else if open_always {
        OPEN_ALWAYS
    } else {
        OPEN_EXISTING
    };
    let mut flags = FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH;
    if directory {
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    let share = if share_delete {
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    } else {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    };
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            share,
            std::ptr::null(),
            disposition,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(TransferError::Storage);
    }
    if windows_handle_is_reparse(handle) {
        unsafe { CloseHandle(handle) };
        return Err(TransferError::UnsafeStorage);
    }
    // SAFETY: CreateFileW returned an owned handle and this File takes it over.
    let file = unsafe { File::from_raw_handle(handle as _) };
    let metadata = file.metadata().map_err(|_| TransferError::Storage)?;
    if directory != metadata.file_type().is_dir() || (!directory && !metadata.file_type().is_file())
    {
        return Err(TransferError::UnsafeStorage);
    }
    Ok(file)
}

#[cfg(windows)]
fn windows_delete_path(path: &Path) -> Result<(), TransferError> {
    let _guard = WindowsPathGuard::open(path)?;
    let file =
        windows_open_path_unchecked_with_sharing(path, false, false, false, false, true, true)?;
    windows_delete_open_file(&file)
}

#[cfg(windows)]
fn windows_delete_open_file(file: &File) -> Result<(), TransferError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };

    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            &info as *const _ as _,
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(TransferError::Storage);
    }
    Ok(())
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

async fn sha256_open_file_with_lease(
    repository: &Repository,
    file: &mut File,
    transfer_id: &str,
    owner: &str,
) -> Result<String, TransferError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| TransferError::Storage)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    let mut since_renewal = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|_| TransferError::Storage)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        since_renewal = since_renewal.saturating_add(read as u64);
        if since_renewal >= 1024 * 1024 {
            renew_repository_lease(repository, transfer_id, owner).await?;
            since_renewal = 0;
            tokio::task::yield_now().await;
        }
    }
    renew_repository_lease(repository, transfer_id, owner).await?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| TransferError::Storage)?;
    Ok(hex::encode(hasher.finalize()))
}

async fn renew_repository_lease(
    repository: &Repository,
    transfer_id: &str,
    owner: &str,
) -> Result<(), TransferError> {
    let _update = sqlx::query(
        "UPDATE c2_transfer_leases SET expires_at = unixepoch() + 30 WHERE transfer_id = ? AND owner = ? AND expires_at > unixepoch()",
    )
    .bind(transfer_id)
    .bind(owner)
    .execute(&repository.pool)
    .await
    .map_err(|_| TransferError::Repository)?;
    Ok(())
}

fn sha256_open_file(file: &mut File) -> Result<String, TransferError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| TransferError::Storage)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer).map_err(|_| TransferError::Storage)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| TransferError::Storage)?;
    Ok(hex::encode(hasher.finalize()))
}

fn sync_directory_handle(directory: &RootHandle) -> Result<(), TransferError> {
    directory
        .file
        .sync_all()
        .map_err(|_| TransferError::Storage)
}
