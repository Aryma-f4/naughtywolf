use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use nw_profile::msgs::{FileAck, FileChunk};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AppError,
    db::{models::FileTransfer, repositories::Repository},
};

pub const DEFAULT_MAX_TRANSFER_BYTES: u64 = 268_435_456;
const CHUNK_BYTES: u64 = 1024;

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
        let next = self.bytes.saturating_add(bytes.len() as u64);
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
    pub fn new(
        repository: Repository,
        evidence_root: impl AsRef<Path>,
        max_bytes: u64,
    ) -> Result<Self, TransferError> {
        let root = evidence_root.as_ref().join("callback-transfers");
        std::fs::create_dir_all(&root).map_err(|_| TransferError::Storage)?;
        let root = root
            .canonicalize()
            .map_err(|_| TransferError::UnsafeStorage)?;
        if !root.is_dir() || max_bytes == 0 {
            return Err(TransferError::UnsafeStorage);
        }
        Ok(Self {
            repository,
            root,
            max_bytes,
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
        std::fs::rename(&stage.part_path, &completed).map_err(|_| TransferError::Storage)?;
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

    pub async fn receive_chunk(
        &self,
        session_id: &str,
        chunk: &FileChunk,
    ) -> Result<FileAck, TransferError> {
        let transfer_id = chunk.transfer_id.ok_or(TransferError::MissingProtocolId)?;
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
            let end = chunk.offset.saturating_add(chunk.data.len() as u64);
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
        if chunk.total > self.max_bytes
            || chunk.offset.saturating_add(chunk.data.len() as u64) > self.max_bytes
        {
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
        if chunk.offset > received {
            return Err(TransferError::OffsetGap {
                expected: received,
                actual: chunk.offset,
            });
        }
        if chunk.offset < received {
            let end = chunk.offset.saturating_add(chunk.data.len() as u64);
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
        let next = received + chunk.data.len() as u64;
        sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND received_bytes = ?")
            .bind(next as i64)
            .bind(&transfer.id)
            .bind(received as i64)
            .execute(&self.repository.pool)
            .await
            .map_err(|_| TransferError::Repository)?;

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
            let completed = self.safe_path(&transfer.storage_key)?;
            std::fs::rename(&part, &completed).map_err(|_| TransferError::Storage)?;
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

    pub async fn ack_upload(&self, session_id: &str, ack: &FileAck) -> Result<(), TransferError> {
        let id = ack.transfer_id.ok_or(TransferError::MissingProtocolId)?;
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
            return Ok(());
        }
        if transfer.status != "active" {
            return Err(TransferError::NotActive);
        }
        if ack.received < current {
            return Ok(());
        }
        let status = if ack.done { "completed" } else { "active" };
        sqlx::query("UPDATE c2_file_transfers SET received_bytes = ?, status = ?, completed_at = CASE WHEN ? THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE completed_at END, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?")
            .bind(ack.received as i64).bind(status).bind(ack.done).bind(&transfer.id)
            .execute(&self.repository.pool).await.map_err(|_| TransferError::Repository)?;
        Ok(())
    }

    async fn activate_if_next(
        &self,
        transfer: &mut FileTransfer,
        direction: &str,
    ) -> Result<(), TransferError> {
        if transfer.status != "queued" {
            return Ok(());
        }
        let next = self
            .repository
            .next_file_transfer(&transfer.session_id, direction)
            .await?;
        if next.as_ref().map(|candidate| candidate.id.as_str()) != Some(transfer.id.as_str()) {
            return Ok(());
        }
        sqlx::query("UPDATE c2_file_transfers SET status = 'active', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND status = 'queued'")
            .bind(&transfer.id).execute(&self.repository.pool).await.map_err(|_| TransferError::Repository)?;
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
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn open_existing_regular(path: &Path) -> Result<File, TransferError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| TransferError::Storage)?;
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
    Ok(hex::encode(hasher.finalize()))
}

fn sync_directory(path: &Path) -> Result<(), TransferError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| TransferError::Storage)
}
