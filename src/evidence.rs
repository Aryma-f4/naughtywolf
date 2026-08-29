use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncWriteExt};

use crate::{
    checks::CheckResult,
    config::Config,
    db::{models::Evidence, repositories::Repository},
    error::AppError,
};

/// Filesystem-backed evidence storage with metadata persisted by `Repository`.
#[derive(Clone)]
pub struct EvidenceStore {
    repository: Repository,
    evidence_dir: PathBuf,
}

impl EvidenceStore {
    pub fn new(repository: Repository, evidence_dir: impl Into<PathBuf>) -> Self {
        Self {
            repository,
            evidence_dir: evidence_dir.into(),
        }
    }

    pub fn from_config(repository: Repository, config: &Config) -> Self {
        Self::new(repository, config.evidence_dir.clone())
    }

    /// Store a bounded blob under a generated path and persist its immutable metadata.
    pub async fn write(
        &self,
        run_id: &str,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<Evidence, AppError> {
        validate_run_id(run_id)?;
        if content_type.trim().is_empty() {
            return Err(AppError::Validation(
                "evidence content type is required".to_owned(),
            ));
        }

        let output_limit = self.repository.output_limit_for_run(run_id).await?;
        if output_limit < 2 {
            return Err(AppError::Validation(
                "catalog output limit must be at least 2 bytes".to_owned(),
            ));
        }
        if bytes.len() > output_limit as usize {
            return Err(AppError::Validation(
                "evidence exceeds the check output limit".to_owned(),
            ));
        }

        let sha256 = hex::encode(Sha256::digest(bytes));
        let filename = format!("{}.json", &sha256[..16]);
        let relative_path = PathBuf::from(run_id).join(filename);
        validate_generated_relative_path(&relative_path)?;
        let storage_path = relative_path.to_string_lossy().into_owned();
        let full_path = self.evidence_dir.join(&relative_path);
        ensure_path_is_within_root(&self.evidence_dir, &full_path)?;

        if let Some(existing) = self
            .repository
            .find_evidence_for_run_at_path(run_id, &storage_path)
            .await?
        {
            self.validate_existing_evidence(
                &existing,
                run_id,
                &relative_path,
                &sha256,
                bytes.len() as i64,
            )
            .await?;
            return Ok(existing);
        }

        ensure_safe_directory(&self.evidence_dir).await?;
        let run_directory = self.evidence_dir.join(run_id);
        ensure_safe_directory(&run_directory).await?;
        reject_existing_path(&full_path).await?;

        // `create_new` maps to exclusive creation. In particular, an existing
        // final symlink is rejected rather than followed on supported platforms.
        let created_file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full_path)
            .await
        {
            Ok(mut file) => {
                file.write_all(bytes)
                    .await
                    .map_err(|_| AppError::Internal)?;
                file.flush().await.map_err(|_| AppError::Internal)?;
                set_owner_only_file_permissions(&full_path).await?;
                true
            }
            Err(_) => return Err(AppError::Internal),
        };

        match self
            .repository
            .create_evidence(
                run_id,
                &storage_path,
                content_type,
                bytes.len() as i64,
                &sha256,
            )
            .await
        {
            Ok(evidence) => Ok(evidence),
            Err(error) => {
                if created_file {
                    let _ = fs::remove_file(&full_path).await;
                }
                Err(error)
            }
        }
    }

    async fn validate_existing_evidence(
        &self,
        evidence: &Evidence,
        run_id: &str,
        expected_relative_path: &Path,
        expected_sha256: &str,
        expected_byte_len: i64,
    ) -> Result<(), AppError> {
        let recorded_relative_path = PathBuf::from(&evidence.storage_path);
        validate_generated_relative_path(&recorded_relative_path)?;
        if evidence.check_run_id != run_id
            || recorded_relative_path != expected_relative_path
            || evidence.sha256 != expected_sha256
            || evidence.byte_len != expected_byte_len
        {
            return Err(AppError::Validation(
                "stored evidence metadata does not match the requested output".to_owned(),
            ));
        }

        self.read_verified(evidence).await.map(|_| ())
    }

    /// Persist the already-bounded JSON representation of a completed check result.
    pub async fn write_check_result(&self, result: &CheckResult) -> Result<Evidence, AppError> {
        let run_id = result.run_id.as_deref().ok_or_else(|| {
            AppError::Validation("check result has no persisted run identifier".to_owned())
        })?;
        let bytes = serde_json::to_vec(result).map_err(|_| AppError::Internal)?;
        self.write(run_id, &bytes, "application/json").await
    }

    /// Read an immutable evidence record after re-validating its generated path,
    /// regular-file metadata, byte length, and SHA-256 checksum.
    pub async fn read_verified(&self, evidence: &Evidence) -> Result<Vec<u8>, AppError> {
        validate_run_id(&evidence.check_run_id)?;
        if evidence.sha256.len() != 64
            || !evidence
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AppError::Validation(
                "stored evidence checksum is invalid".to_owned(),
            ));
        }

        let recorded_relative_path = PathBuf::from(&evidence.storage_path);
        validate_generated_relative_path(&recorded_relative_path)?;
        let expected_relative_path =
            PathBuf::from(&evidence.check_run_id).join(format!("{}.json", &evidence.sha256[..16]));
        if recorded_relative_path != expected_relative_path {
            return Err(AppError::Validation(
                "stored evidence path does not match its metadata".to_owned(),
            ));
        }

        let full_path = self.evidence_dir.join(&recorded_relative_path);
        ensure_path_is_within_root(&self.evidence_dir, &full_path)?;
        require_safe_directory(&self.evidence_dir).await?;
        require_safe_directory(&self.evidence_dir.join(&evidence.check_run_id)).await?;
        read_verified_file(&full_path, evidence).await
    }
}

fn validate_run_id(run_id: &str) -> Result<(), AppError> {
    if run_id.is_empty()
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(AppError::Validation(
            "invalid evidence run identifier".to_owned(),
        ));
    }
    Ok(())
}

fn validate_generated_relative_path(path: &Path) -> Result<(), AppError> {
    if path.is_absolute()
        || path.components().any(
            |component| !matches!(component, Component::Normal(component) if !component.is_empty()),
        )
    {
        return Err(AppError::Validation(
            "invalid generated evidence path".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_path_is_within_root(root: &Path, path: &Path) -> Result<(), AppError> {
    if !path.starts_with(root) {
        return Err(AppError::Validation(
            "invalid generated evidence path".to_owned(),
        ));
    }
    Ok(())
}

async fn ensure_safe_directory(path: &Path) -> Result<(), AppError> {
    reject_symlink_path(path).await?;
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(AppError::Validation(
                "evidence directory is not a safe directory".to_owned(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .await
                .map_err(|_| AppError::Internal)?;
        }
        Err(_) => return Err(AppError::Internal),
    }
    reject_symlink_path(path).await?;
    set_owner_only_directory_permissions(path).await
}

async fn require_safe_directory(path: &Path) -> Result<(), AppError> {
    reject_symlink_path(path).await?;
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata.is_dir() => Err(AppError::Validation(
            "evidence directory is not a safe directory".to_owned(),
        )),
        Ok(_) => set_owner_only_directory_permissions(path).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(AppError::Validation(
            "evidence directory is missing".to_owned(),
        )),
        Err(_) => Err(AppError::Internal),
    }
}

async fn read_verified_file(path: &Path, evidence: &Evidence) -> Result<Vec<u8>, AppError> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::Validation("stored evidence file is missing".to_owned())
        } else {
            AppError::Internal
        }
    })?;
    if !metadata.file_type().is_file() || metadata.len() != evidence.byte_len as u64 {
        return Err(AppError::Validation(
            "stored evidence file does not match its metadata".to_owned(),
        ));
    }

    let bytes = fs::read(path).await.map_err(|_| AppError::Internal)?;
    if hex::encode(Sha256::digest(&bytes)) != evidence.sha256 {
        return Err(AppError::Validation(
            "stored evidence file does not match its checksum".to_owned(),
        ));
    }
    Ok(bytes)
}

async fn reject_existing_path(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Err(AppError::Validation(
            "generated evidence path already exists".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AppError::Internal),
    }
}

async fn reject_symlink_path(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::Validation(
            "evidence path contains a symlink".to_owned(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AppError::Internal),
    }
}

async fn set_owner_only_directory_permissions(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| AppError::Internal)?;
    }
    Ok(())
}

async fn set_owner_only_file_permissions(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|_| AppError::Internal)?;
    }
    Ok(())
}
