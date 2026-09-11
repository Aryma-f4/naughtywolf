use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
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
    max_bytes: u64,
    locks: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

pub struct UploadStage {
    file: Option<File>,
    part_path: PathBuf,
    storage_key: String,
    bytes: u64,
    max_bytes: u64,
    hasher: Sha256,
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
            let _ = std::fs::remove_file(&self.part_path);
        }
    }
}

impl TransferStore {
    pub fn from_config(
        repository: Repository,
        config: &crate::config::Config,
    ) -> Result<Self, TransferError> {
        Self::new(repository, &config.evidence_dir, config.max_transfer_bytes)
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
    pub fn new(
        repository: Repository,
        evidence_root: impl AsRef<Path>,
        max_bytes: u64,
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
        Ok(Self {
            repository,
            root,
            max_bytes,
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
        Self::new(repository, evidence_root, max_bytes)
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
        let part_path = self.safe_path(&format!("{storage_key}.part"))?;
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&part_path)
            .map_err(|_| TransferError::Storage)?;
        sync_directory(&self.root)?;
        Ok(UploadStage {
            file: Some(file),
            part_path,
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
        let completed = self.safe_path(&stage.storage_key)?;
        publish_no_replace(&stage.part_path, &completed)?;
        sync_directory(&self.root)?;
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
        let active_ids: std::collections::HashSet<_> = transfers
            .iter()
            .filter(|transfer| matches!(transfer.status.as_str(), "queued" | "active"))
            .map(|transfer| transfer.id.clone())
            .collect();
        self.locks
            .lock()
            .expect("transfer lock map poisoned")
            .retain(|id, _| active_ids.contains(id));
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
        let retained_downloads: std::collections::HashSet<_> = transfers
            .iter()
            .filter(|transfer| transfer.direction == "download" && transfer.status == "completed")
            .map(|transfer| transfer.storage_key.clone())
            .collect();
        let entries = std::fs::read_dir(&self.root).map_err(|_| TransferError::Storage)?;
        for entry in entries {
            let entry = entry.map_err(|_| TransferError::Storage)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !live.contains(&name) && !retained_downloads.contains(&name) {
                let path = self.safe_path(&name)?;
                let metadata =
                    std::fs::symlink_metadata(&path).map_err(|_| TransferError::Storage)?;
                if metadata.file_type().is_file() {
                    std::fs::remove_file(path).map_err(|_| TransferError::Storage)?;
                }
            }
        }
        sync_directory(&self.root)
    }

    pub async fn receive_chunk(
        &self,
        session_id: &str,
        chunk: &FileChunk,
    ) -> Result<FileAck, TransferError> {
        let transfer_id = chunk.transfer_id.ok_or(TransferError::MissingProtocolId)?;
        let lock = self.lock_for(&transfer_id.to_string());
        let _guard = lock.lock().await;
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
            let completed = self.safe_path(&transfer.storage_key)?;
            let mut file = open_existing_regular(&completed)?;
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
        let part = self.safe_path(&format!("{}.part", transfer.storage_key))?;
        let completed = self.safe_path(&transfer.storage_key)?;
        if reconcile_completed_download(
            &self.repository,
            &mut transfer,
            &completed,
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
            let mut file = open_existing_regular(&part)?;
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

        let mut file = if received == 0 && !part.exists() {
            OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .open(&part)
                .map_err(|_| TransferError::Storage)?
        } else {
            open_existing_regular(&part)?
        };
        let metadata = file.metadata().map_err(|_| TransferError::Storage)?;
        if !metadata.file_type().is_file() {
            return Err(TransferError::UnsafeStorage);
        }
        file.set_len(received).map_err(|_| TransferError::Storage)?;
        file.seek(SeekFrom::Start(received))
            .map_err(|_| TransferError::Storage)?;
        file.write_all(&chunk.data)
            .map_err(|_| TransferError::Storage)?;
        file.sync_all().map_err(|_| TransferError::Storage)?;
        sync_directory(&self.root)?;
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
            let digest = sha256_file(&part)?;
            if transfer
                .sha256
                .as_deref()
                .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest))
            {
                self.fail(&transfer.id, "SHA-256 mismatch").await?;
                return Err(TransferError::ChecksumMismatch);
            }
            publish_no_replace(&part, &completed)?;
            sync_directory(&self.root)?;
            sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, sha256 = ?, status = 'completed', error = NULL, completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?")
                .bind(next as i64)
                .bind(&digest)
                .bind(&transfer.id)
                .execute(&self.repository.pool)
                .await
                .map_err(|_| TransferError::Repository)?;
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
        let path = self.safe_path(&transfer.storage_key)?;
        let mut file = open_existing_regular(&path)?;
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
        let path = self.safe_path(&transfer.storage_key)?;
        let mut file = open_existing_regular(&path)?;
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
        let Some(mut transfer) = self
            .repository
            .next_file_transfer(session_id, "upload")
            .await?
        else {
            return Ok(Vec::new());
        };
        let lock = self.lock_for(&transfer.id);
        let _guard = lock.lock().await;
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
        let path = self.safe_path(&transfer.storage_key)?;
        let mut file = open_existing_regular(&path)?;
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
        let status = if ack.done { "completed" } else { "active" };
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
        sqlx::query("UPDATE c2_file_transfers SET status = 'error', error = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?")
            .bind(error).bind(transfer_id).execute(&self.repository.pool).await.map_err(|_| TransferError::Repository)?;
        Ok(())
    }

    fn safe_path(&self, file_name: &str) -> Result<PathBuf, TransferError> {
        if file_name.is_empty()
            || file_name.contains(['/', '\\'])
            || file_name == "."
            || file_name == ".."
        {
            return Err(TransferError::UnsafeStorage);
        }
        let path = self.root.join(file_name);
        let parent = path
            .parent()
            .ok_or(TransferError::UnsafeStorage)?
            .canonicalize()
            .map_err(|_| TransferError::UnsafeStorage)?;
        if parent != self.root {
            return Err(TransferError::UnsafeStorage);
        }
        if path.exists() {
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|_| TransferError::UnsafeStorage)?;
            if !metadata.file_type().is_file()
                || path
                    .canonicalize()
                    .ok()
                    .and_then(|resolved| resolved.parent().map(Path::to_path_buf))
                    != Some(self.root.clone())
            {
                return Err(TransferError::UnsafeStorage);
            }
        }
        Ok(path)
    }

    fn lock_for(&self, transfer_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().expect("transfer lock map poisoned");
        locks
            .entry(transfer_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn reconcile_completed_download(
    repository: &Repository,
    transfer: &mut FileTransfer,
    completed: &Path,
    total: u64,
    max_bytes: u64,
) -> Result<bool, TransferError> {
    if !completed.exists() {
        return Ok(false);
    }
    let metadata = std::fs::symlink_metadata(completed).map_err(|_| TransferError::Storage)?;
    if !metadata.file_type().is_file() || metadata.len() != total || total > max_bytes {
        return Err(TransferError::UnsafeStorage);
    }
    let digest = sha256_file(completed)?;
    if transfer
        .sha256
        .as_deref()
        .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest))
    {
        return Err(TransferError::ChecksumMismatch);
    }
    let update = sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, sha256 = ?, status = 'completed', error = NULL, completed_at = COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND status IN ('queued', 'active')")
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
    transfer.status = "completed".to_owned();
    Ok(true)
}

fn publish_no_replace(part: &Path, completed: &Path) -> Result<(), TransferError> {
    // hard_link is an atomic create-without-replace on the local filesystems
    // supported by the server. Remove the staged name only after publication.
    let metadata = std::fs::symlink_metadata(part).map_err(|_| TransferError::Storage)?;
    if !metadata.file_type().is_file() {
        return Err(TransferError::UnsafeStorage);
    }
    std::fs::hard_link(part, completed).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            TransferError::UnsafeStorage
        } else {
            TransferError::Storage
        }
    })?;
    std::fs::remove_file(part).map_err(|_| TransferError::Storage)
}

fn open_existing_regular(path: &Path) -> Result<File, TransferError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| TransferError::Storage)?;
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

fn sha256_file(path: &Path) -> Result<String, TransferError> {
    let mut file = open_existing_regular(path)?;
    sha256_open_file(&mut file)
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

fn sync_directory(path: &Path) -> Result<(), TransferError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| TransferError::Storage)
}
