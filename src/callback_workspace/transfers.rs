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
    bytes: u64,
    max_bytes: u64,
    hasher: Sha256,
}

struct RootHandle {
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
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
        std::fs::create_dir_all(&root).map_err(|_| TransferError::Storage)?;
        let root = root
            .canonicalize()
            .map_err(|_| TransferError::UnsafeStorage)?;
        if !root.is_dir() || max_bytes == 0 || max_bytes > i64::MAX as u64 {
            return Err(TransferError::UnsafeStorage);
        }
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
        let file = create_new_relative(&self.root_dir, &format!("{storage_key}.part"))?;
        sync_directory_handle(&self.root_dir)?;
        Ok(UploadStage {
            file: Some(file),
            root_dir: self.root_dir.clone(),
            storage_key,
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
        drop(file);
        publish_no_replace_at(
            &stage.root_dir,
            &format!("{}.part", stage.storage_key),
            &stage.storage_key,
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
                ]
            })
            .collect();
        let retained_completed: std::collections::HashSet<_> = transfers
            .iter()
            .filter(|transfer| transfer.status == "completed")
            .map(|transfer| transfer.storage_key.clone())
            .collect();
        let entries = std::fs::read_dir(&self.root).map_err(|_| TransferError::Storage)?;
        for entry in entries {
            let entry = entry.map_err(|_| TransferError::Storage)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !live.contains(&name) && !retained_completed.contains(&name) {
                if let Ok(file) = open_existing_regular_at(&self.root_dir, &name, false) {
                    drop(file);
                    unlink_relative(&self.root_dir, &name)?;
                }
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
            let lease = self.acquire_lease(&transfer.id).await?;
            for name in [
                transfer.storage_key.clone(),
                format!("{}.part", transfer.storage_key),
            ] {
                let path = self.safe_path(&name)?;
                if std::fs::symlink_metadata(&path)
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false)
                {
                    std::fs::remove_file(path).map_err(|_| TransferError::Storage)?;
                }
            }
            sqlx::query("DELETE FROM c2_file_transfers WHERE id = ? AND status IN ('completed','error','cancelled')")
                .bind(&transfer.id)
                .execute(&self.repository.pool)
                .await
                .map_err(|_| TransferError::Repository)?;
            self.release_lease(&transfer.id, &lease).await?;
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
        let lease = self.acquire_lease(&transfer_id.to_string()).await?;
        let result = self
            .receive_chunk_inner(session_id, chunk, transfer_id)
            .await;
        self.release_lease(&transfer_id.to_string(), &lease).await?;
        result
    }

    async fn receive_chunk_inner(
        &self,
        session_id: &str,
        chunk: &FileChunk,
        transfer_id: Uuid,
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
        file.set_len(received).map_err(|_| TransferError::Storage)?;
        file.seek(SeekFrom::Start(received))
            .map_err(|_| TransferError::Storage)?;
        file.write_all(&chunk.data)
            .map_err(|_| TransferError::Storage)?;
        file.sync_all().map_err(|_| TransferError::Storage)?;
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
            let mut verify = open_existing_regular_at(&self.root_dir, &part_name, false)?;
            let digest = sha256_open_file(&mut verify)?;
            if transfer
                .sha256
                .as_deref()
                .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest))
            {
                self.fail(&transfer.id, "SHA-256 mismatch").await?;
                return Err(TransferError::ChecksumMismatch);
            }
            publish_no_replace_at(&self.root_dir, &part_name, &transfer.storage_key)?;
            sync_directory_handle(&self.root_dir)?;
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
            .next_upload_chunks_inner(session_id, budget, transfer)
            .await;
        self.release_lease(&transfer_id, &lease).await?;
        result
    }

    async fn next_upload_chunks_inner(
        &self,
        _session_id: &str,
        budget: usize,
        mut transfer: FileTransfer,
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
        let update = sqlx::query("UPDATE c2_file_transfers SET status = 'error', error = ?, completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND status IN ('queued', 'active')")
            .bind(error).bind(transfer_id).execute(&self.repository.pool).await.map_err(|_| TransferError::Repository)?;
        if update.rows_affected() != 1 {
            return Err(TransferError::Repository);
        }
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
    let digest = sha256_open_file(&mut completed)?;
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
    #[cfg(not(unix))]
    {
        let part_path = root_path(root, part)?;
        let completed_path = root_path(root, completed)?;
        std::fs::hard_link(part_path, completed_path).map_err(|_| TransferError::Storage)?;
        std::fs::remove_file(part_path).map_err(|_| TransferError::Storage)
    }
}

fn open_directory(path: &Path) -> Result<RootHandle, TransferError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| TransferError::UnsafeStorage)?;
        if !file
            .metadata()
            .map_err(|_| TransferError::UnsafeStorage)?
            .file_type()
            .is_dir()
        {
            return Err(TransferError::UnsafeStorage);
        }
        Ok(RootHandle {
            file,
            #[cfg(not(unix))]
            path: path.to_path_buf(),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(RootHandle {
            file: File::open(path).map_err(|_| TransferError::UnsafeStorage)?,
            #[cfg(not(unix))]
            path: path.to_path_buf(),
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
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(root_path(root, name)?)
            .map_err(|_| TransferError::Storage)
    }
}

fn unlink_relative(root: &RootHandle, name: &str) -> Result<(), TransferError> {
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
    #[cfg(not(unix))]
    {
        match std::fs::remove_file(root_path(root, name)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(TransferError::Storage),
        }
    }
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
    #[cfg(not(unix))]
    {
        let mut options = OpenOptions::new();
        options.read(true).write(write);
        options
            .open(root_path(root, name)?)
            .map_err(|_| TransferError::Storage)
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

#[cfg(not(unix))]
fn root_path(root: &RootHandle, name: &str) -> Result<PathBuf, TransferError> {
    validate_component(name)?;
    Ok(root.path.join(name))
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
