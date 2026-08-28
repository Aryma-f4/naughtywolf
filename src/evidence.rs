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

        if let Some(existing) = self
            .repository
            .find_evidence_for_run_at_path(run_id, &storage_path)
            .await?
        {
            return Ok(existing);
        }

        let run_directory = self.evidence_dir.join(run_id);
        let full_path = self.evidence_dir.join(&relative_path);
        ensure_path_is_within_root(&self.evidence_dir, &full_path)?;
        fs::create_dir_all(&run_directory)
            .await
            .map_err(|_| AppError::Internal)?;
        set_owner_only_directory_permissions(&run_directory).await?;

        let mut created_file = false;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full_path)
            .await
        {
            Ok(mut file) => {
                created_file = true;
                file.write_all(bytes)
                    .await
                    .map_err(|_| AppError::Internal)?;
                file.flush().await.map_err(|_| AppError::Internal)?;
                set_owner_only_file_permissions(&full_path).await?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(AppError::Internal),
        }

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

    /// Persist the already-bounded JSON representation of a completed check result.
    pub async fn write_check_result(&self, result: &CheckResult) -> Result<Evidence, AppError> {
        let run_id = result.run_id.as_deref().ok_or_else(|| {
            AppError::Validation("check result has no persisted run identifier".to_owned())
        })?;
        let bytes = serde_json::to_vec(result).map_err(|_| AppError::Internal)?;
        self.write(run_id, &bytes, "application/json").await
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
