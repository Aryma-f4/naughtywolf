use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::{
    auth::AuthenticatedUser,
    checks::catalog::{CheckDefinition, catalog},
    db::{
        models::{Asset, RunState},
        repositories::Repository,
    },
    error::AppError,
    policy::authorize_asset_run,
};

#[derive(Clone, Default)]
pub struct CancellationToken {
    inner: Arc<CancellationState>,
}

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        let notified = self.inner.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRecordReviewInput {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", content = "input", rename_all = "kebab-case")]
pub enum CheckInput {
    AssetRecordReview(AssetRecordReviewInput),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFinding {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub run_id: Option<String>,
    pub state: RunState,
    pub findings: Vec<CheckFinding>,
    pub error: Option<String>,
    pub output_truncated: bool,
}

impl CheckResult {
    pub fn succeeded(findings: Vec<CheckFinding>) -> Self {
        Self {
            run_id: None,
            state: RunState::Succeeded,
            findings,
            error: None,
            output_truncated: false,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            run_id: None,
            state: RunState::Failed,
            findings: Vec::new(),
            error: Some(error.into()),
            output_truncated: false,
        }
    }

    pub fn cancelled() -> Self {
        Self {
            run_id: None,
            state: RunState::Cancelled,
            findings: Vec::new(),
            error: Some("check cancelled".to_owned()),
            output_truncated: false,
        }
    }

    pub fn truncate(mut self, max_output_bytes: usize) -> Self {
        if serialized_len(&self) <= max_output_bytes {
            return self;
        }

        self.output_truncated = true;
        while !self.findings.is_empty() && serialized_len(&self) > max_output_bytes {
            self.findings.pop();
        }
        if serialized_len(&self) > max_output_bytes {
            self.error = None;
        }
        self
    }
}

fn serialized_len(result: &CheckResult) -> usize {
    serde_json::to_vec(result).map_or(usize::MAX, |bytes| bytes.len())
}

#[derive(Clone)]
pub struct Runner {
    repository: Option<Repository>,
    user: Option<AuthenticatedUser>,
    cancellation: CancellationToken,
    test_timeout: Option<Duration>,
}

impl Runner {
    pub fn new(repository: Repository, user: AuthenticatedUser) -> Self {
        Self::with_cancellation(repository, user, CancellationToken::new())
    }

    pub fn with_cancellation(
        repository: Repository,
        user: AuthenticatedUser,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            repository: Some(repository),
            user: Some(user),
            cancellation,
            test_timeout: None,
        }
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub async fn start(
        &self,
        asset: Asset,
        check_id: &str,
        input: CheckInput,
    ) -> Result<CheckResult, AppError> {
        let repository = self.repository.as_ref().ok_or(AppError::Internal)?;
        let user = self.user.as_ref().ok_or(AppError::Internal)?;

        authorize_asset_run(repository, user, &asset.id).await?;
        let asset = repository
            .find_asset(&asset.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let definition = catalog()
            .iter()
            .find(|definition| definition.id == check_id)
            .ok_or_else(|| AppError::Validation("unknown check".to_owned()))?;
        user.require(definition.required_role)?;
        validate_input(definition, &input)?;

        repository
            .ensure_builtin_check(
                definition.id,
                definition.label,
                &definition.required_role.to_string(),
                definition.timeout.as_secs() as i64,
                definition.max_output_bytes as i64,
            )
            .await?;
        let input_json = serde_json::to_value(&input).map_err(|_| AppError::Internal)?;
        let run = repository
            .create_run(
                definition.id,
                &asset.id,
                &asset.operation_id,
                Some(&user.id),
                &input_json,
            )
            .await?;
        repository.start_run(&run.id).await?;

        let mut result = self.execute(definition, asset, input).await;
        if self.cancellation.is_cancelled() {
            result = CheckResult::cancelled();
        }
        result.run_id = Some(run.id.clone());
        result = result.truncate(definition.max_output_bytes);
        let result_json = serde_json::to_value(&result).map_err(|_| AppError::Internal)?;
        repository
            .finish_run(
                &run.id,
                result.state,
                Some(&result_json),
                result.error.as_deref(),
                result.output_truncated,
            )
            .await?;

        Ok(result)
    }

    async fn execute(
        &self,
        definition: &CheckDefinition,
        asset: Asset,
        input: CheckInput,
    ) -> CheckResult {
        let work = run_check(definition.id, asset, input);
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => CheckResult::cancelled(),
            outcome = tokio::time::timeout(definition.timeout, work) => match outcome {
                Ok(Ok(result)) => result,
                Ok(Err(message)) => CheckResult::failed(message),
                Err(_) => CheckResult::failed("check timed out"),
            },
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            repository: None,
            user: None,
            cancellation: CancellationToken::new(),
            test_timeout: Some(timeout),
        }
    }

    #[doc(hidden)]
    pub async fn run_for_test(&self, check_id: &str) -> CheckResult {
        let timeout = self.test_timeout.unwrap_or(Duration::from_secs(5));
        let work = async {
            match check_id {
                "sleeping-check" => tokio::time::sleep(Duration::from_secs(60)).await,
                _ => return CheckResult::failed("unknown test check"),
            }
            CheckResult::succeeded(Vec::new())
        };

        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => CheckResult::cancelled(),
            outcome = tokio::time::timeout(timeout, work) => match outcome {
                Ok(result) => result,
                Err(_) => CheckResult::failed("check timed out"),
            },
        }
    }
}

fn validate_input(definition: &CheckDefinition, input: &CheckInput) -> Result<(), AppError> {
    match (definition.id, input) {
        ("asset-record-review", CheckInput::AssetRecordReview(_)) => Ok(()),
        _ => Err(AppError::Validation(
            "input does not match the selected check".to_owned(),
        )),
    }
}

async fn run_check(
    check_id: &str,
    asset: Asset,
    input: CheckInput,
) -> Result<CheckResult, &'static str> {
    match (check_id, input) {
        ("asset-record-review", CheckInput::AssetRecordReview(_)) => review_asset_record(asset),
        _ => Err("check implementation is unavailable"),
    }
}

fn review_asset_record(asset: Asset) -> Result<CheckResult, &'static str> {
    if asset.owner.trim().is_empty() {
        return Err("approved asset record is missing owner");
    }
    if asset.kind.trim().is_empty() {
        return Err("approved asset record is missing type");
    }
    if asset.address.trim().is_empty() {
        return Err("approved asset record is missing address");
    }

    Ok(CheckResult::succeeded(vec![CheckFinding {
        code: "asset-record-complete".to_owned(),
        message: "Approved asset owner, type, and address are present.".to_owned(),
    }]))
}
