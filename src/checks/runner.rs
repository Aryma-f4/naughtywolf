use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{
    Deserialize, Serialize, Serializer,
    ser::{SerializeMap, SerializeStruct},
};
use tokio::sync::watch;
use uuid::Uuid;

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

#[derive(Clone)]
struct CancellationToken {
    sender: watch::Sender<bool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self { sender }
    }
}

impl CancellationToken {
    fn new() -> Self {
        Self::default()
    }

    fn cancel(&self) {
        self.sender.send_replace(true);
    }

    fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRecordReviewInput {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", content = "input", rename_all = "kebab-case")]
pub enum CheckInput {
    AssetRecordReview(AssetRecordReviewInput),
    SurfaceRecon(super::recon::SurfaceReconInput),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFinding {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckResult {
    pub run_id: Option<String>,
    pub state: RunState,
    pub findings: Vec<CheckFinding>,
    pub error: Option<String>,
    pub output_truncated: bool,
    #[serde(skip)]
    bounded_fallback: bool,
}

impl Serialize for CheckResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.bounded_fallback {
            return serializer.serialize_map(Some(0))?.end();
        }

        let mut state = serializer.serialize_struct("CheckResult", 5)?;
        state.serialize_field("run_id", &self.run_id)?;
        state.serialize_field("state", &self.state)?;
        state.serialize_field("findings", &self.findings)?;
        state.serialize_field("error", &self.error)?;
        state.serialize_field("output_truncated", &self.output_truncated)?;
        state.end()
    }
}

impl CheckResult {
    pub fn succeeded(findings: Vec<CheckFinding>) -> Self {
        Self {
            run_id: None,
            state: RunState::Succeeded,
            findings,
            error: None,
            output_truncated: false,
            bounded_fallback: false,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            run_id: None,
            state: RunState::Failed,
            findings: Vec::new(),
            error: Some(error.into()),
            output_truncated: false,
            bounded_fallback: false,
        }
    }

    pub fn cancelled() -> Self {
        Self {
            run_id: None,
            state: RunState::Cancelled,
            findings: Vec::new(),
            error: Some("check cancelled".to_owned()),
            output_truncated: false,
            bounded_fallback: false,
        }
    }

    pub fn truncate(mut self, max_output_bytes: usize) -> Self {
        let max_output_bytes = max_output_bytes.max(2);
        if serialized_len(&self) <= max_output_bytes {
            return self;
        }

        self.output_truncated = true;
        while !self.findings.is_empty() && serialized_len(&self) > max_output_bytes {
            self.findings.pop();
        }
        if serialized_len(&self) > max_output_bytes {
            self.run_id = None;
            self.state = RunState::Failed;
            self.error = Some("check output exceeded limit".to_owned());
        }
        if serialized_len(&self) > max_output_bytes {
            self.bounded_fallback = true;
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
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    test_timeout: Option<Duration>,
    execution_delay: Option<Duration>,
}

impl Runner {
    pub fn new(repository: Repository, user: AuthenticatedUser) -> Self {
        Self {
            repository: Some(repository),
            user: Some(user),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            test_timeout: None,
            execution_delay: None,
        }
    }

    #[doc(hidden)]
    pub fn with_execution_delay(
        repository: Repository,
        user: AuthenticatedUser,
        delay: Duration,
    ) -> Self {
        let mut runner = Self::new(repository, user);
        runner.execution_delay = Some(delay);
        runner
    }

    pub fn cancel(&self, run_id: &str) -> Result<(), AppError> {
        let cancellations = self.cancellations.lock().map_err(|_| AppError::Internal)?;
        let cancellation = cancellations.get(run_id).ok_or(AppError::NotFound)?;
        cancellation.cancel();
        Ok(())
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
        let cancellation = self.register_run(&run.id)?;
        repository.start_run(&run.id).await?;

        let mut result = self
            .execute(definition, asset, input, cancellation.token())
            .await;
        if cancellation.finalize()? {
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
        cancellation: &CancellationToken,
    ) -> CheckResult {
        let work = async {
            if let Some(delay) = self.execution_delay {
                tokio::time::sleep(delay).await;
            }
            run_check(definition.id, asset, input).await
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => CheckResult::cancelled(),
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
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            test_timeout: Some(timeout),
            execution_delay: None,
        }
    }

    #[doc(hidden)]
    pub async fn run_for_test(&self, check_id: &str) -> CheckResult {
        let run_id = Uuid::new_v4().to_string();
        self.run_test_check(&run_id, check_id).await
    }

    #[doc(hidden)]
    pub async fn run_cancel_before_wait_for_test(&self, run_id: &str) -> CheckResult {
        let cancellation = match self.register_run(run_id) {
            Ok(cancellation) => cancellation,
            Err(_) => return CheckResult::failed("test run registration failed"),
        };
        if self.cancel(run_id).is_err() {
            return CheckResult::failed("test run cancellation failed");
        }
        let mut result =
            tokio::time::timeout(Duration::from_millis(20), cancellation.token().cancelled())
                .await
                .map_or_else(
                    |_| CheckResult::failed("cancellation was not retained"),
                    |_| CheckResult::cancelled(),
                );
        let _ = cancellation.finalize();
        result.run_id = Some(run_id.to_owned());
        result
    }

    async fn run_test_check(&self, run_id: &str, check_id: &str) -> CheckResult {
        let cancellation = match self.register_run(run_id) {
            Ok(cancellation) => cancellation,
            Err(_) => return CheckResult::failed("test run registration failed"),
        };
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
            _ = cancellation.token().cancelled() => CheckResult::cancelled(),
            outcome = tokio::time::timeout(timeout, work) => match outcome {
                Ok(result) => result,
                Err(_) => CheckResult::failed("check timed out"),
            },
        }
    }

    fn register_run(&self, run_id: &str) -> Result<RunCancellation, AppError> {
        let token = CancellationToken::new();
        let mut cancellations = self.cancellations.lock().map_err(|_| AppError::Internal)?;
        match cancellations.entry(run_id.to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(token.clone());
            }
            Entry::Occupied(_) => {
                return Err(AppError::Conflict(
                    "check run is already registered".to_owned(),
                ));
            }
        }
        Ok(RunCancellation {
            run_id: run_id.to_owned(),
            token,
            cancellations: self.cancellations.clone(),
            active: true,
        })
    }
}

struct RunCancellation {
    run_id: String,
    token: CancellationToken,
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    active: bool,
}

impl RunCancellation {
    fn token(&self) -> &CancellationToken {
        &self.token
    }

    fn finalize(mut self) -> Result<bool, AppError> {
        let mut cancellations = self.cancellations.lock().map_err(|_| AppError::Internal)?;
        let cancelled = self.token.is_cancelled();
        cancellations.remove(&self.run_id);
        self.active = false;
        Ok(cancelled)
    }
}

impl Drop for RunCancellation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Ok(mut cancellations) = self.cancellations.lock() {
            cancellations.remove(&self.run_id);
        }
    }
}

fn validate_input(definition: &CheckDefinition, input: &CheckInput) -> Result<(), AppError> {
    match (definition.id, input) {
        ("asset-record-review", CheckInput::AssetRecordReview(_)) => Ok(()),
        ("surface-recon", CheckInput::SurfaceRecon(_)) => Ok(()),
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
        ("surface-recon", CheckInput::SurfaceRecon(input)) => {
            super::recon::collect(&asset.address, input).await
        }
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
