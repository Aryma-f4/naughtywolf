use std::time::Duration;

use naughtywolf::{
    AppError,
    auth::{AuthenticatedUser, rbac::Role},
    checks::{
        AssetRecordReviewInput, CancellationToken, CheckFinding, CheckInput, CheckResult,
        ExecutionKind, RunState, Runner, catalog,
    },
    db::{self, models::CheckRun, repositories::Repository},
};

struct RunnerFixture {
    repo: Repository,
    user: AuthenticatedUser,
    asset: naughtywolf::db::models::Asset,
}

async fn runner_fixture(role: Role) -> RunnerFixture {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repo = Repository { pool };
    let operation = repo
        .create_operation("Runner lab", "Bounded runner tests")
        .await
        .unwrap();
    sqlx::query("UPDATE operations SET status = 'active' WHERE id = ?")
        .bind(&operation.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    let asset = repo
        .create_asset(&operation.id, "web-01", "host", "security-team", "10.0.0.8")
        .await
        .unwrap();
    let user = AuthenticatedUser {
        id: "runner-user".to_owned(),
        username: "runner-user".to_owned(),
        role,
    };
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind(&user.id)
        .bind(&user.username)
        .bind("not-a-real-password-hash")
        .bind(role.to_string())
        .execute(&repo.pool)
        .await
        .unwrap();
    repo.add_member(&operation.id, &user.id).await.unwrap();

    RunnerFixture { repo, user, asset }
}

fn asset_record_input() -> CheckInput {
    CheckInput::AssetRecordReview(AssetRecordReviewInput::default())
}

#[test]
fn catalog_has_only_builtin_non_destructive_checks() {
    let checks = catalog();

    assert_eq!(checks.len(), 1);
    assert!(
        checks
            .iter()
            .all(|check| check.builtin && !check.mutates_target)
    );
}

#[tokio::test]
async fn runner_marks_timed_out_work_as_failed() {
    let result = Runner::with_timeout(Duration::from_millis(1))
        .run_for_test("sleeping-check")
        .await;

    assert_eq!(result.state, RunState::Failed);
    assert_eq!(result.error.as_deref(), Some("check timed out"));
}

#[test]
fn initial_catalog_does_not_require_subprocess_execution() {
    assert!(
        catalog()
            .iter()
            .all(|check| check.execution_kind == ExecutionKind::InProcess)
    );
}

#[test]
fn asset_record_input_rejects_command_fields() {
    let result = serde_json::from_value::<AssetRecordReviewInput>(serde_json::json!({
        "command": "whoami"
    }));

    assert!(result.is_err());
}

#[test]
fn run_states_allow_only_forward_lifecycle_transitions() {
    assert!(RunState::Queued.can_transition_to(RunState::Running));
    assert!(RunState::Running.can_transition_to(RunState::Succeeded));
    assert!(RunState::Running.can_transition_to(RunState::Failed));
    assert!(RunState::Running.can_transition_to(RunState::Cancelled));
    assert!(!RunState::Queued.can_transition_to(RunState::Succeeded));
    assert!(!RunState::Succeeded.can_transition_to(RunState::Running));
}

#[test]
fn check_results_drop_oversized_findings_within_the_output_bound() {
    let result = CheckResult::succeeded(vec![CheckFinding {
        code: "large-finding".to_owned(),
        message: "x".repeat(10_000),
    }])
    .truncate(256);

    assert!(result.output_truncated);
    assert!(serde_json::to_vec(&result).unwrap().len() <= 256);
}

#[tokio::test]
async fn authorized_asset_review_persists_running_and_success_states() {
    let fixture = runner_fixture(Role::Operator).await;
    let result = Runner::new(fixture.repo.clone(), fixture.user)
        .start(fixture.asset, "asset-record-review", asset_record_input())
        .await
        .unwrap();

    assert_eq!(result.state, RunState::Succeeded);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].code, "asset-record-complete");
    let run = sqlx::query_as::<_, CheckRun>("SELECT * FROM check_runs WHERE id = ?")
        .bind(result.run_id.as_deref().unwrap())
        .fetch_one(&fixture.repo.pool)
        .await
        .unwrap();
    assert_eq!(run.state, RunState::Succeeded);
    assert!(run.started_at.is_some());
    assert!(run.finished_at.is_some());
    assert_eq!(run.result_json.unwrap()["state"], "succeeded");
}

#[tokio::test]
async fn runner_uses_the_authorized_persisted_asset_record() {
    let fixture = runner_fixture(Role::Operator).await;
    let mut untrusted_asset = fixture.asset.clone();
    untrusted_asset.owner = "".to_owned();

    let result = Runner::new(fixture.repo, fixture.user)
        .start(untrusted_asset, "asset-record-review", asset_record_input())
        .await
        .unwrap();

    assert_eq!(result.state, RunState::Succeeded);
}

#[tokio::test]
async fn incomplete_approved_asset_fails_and_persists_the_safe_error() {
    let fixture = runner_fixture(Role::Operator).await;
    sqlx::query("UPDATE assets SET owner = '   ' WHERE id = ?")
        .bind(&fixture.asset.id)
        .execute(&fixture.repo.pool)
        .await
        .unwrap();

    let result = Runner::new(fixture.repo.clone(), fixture.user)
        .start(fixture.asset, "asset-record-review", asset_record_input())
        .await
        .unwrap();

    assert_eq!(result.state, RunState::Failed);
    assert_eq!(
        result.error.as_deref(),
        Some("approved asset record is missing owner")
    );
    let persisted_error: String =
        sqlx::query_scalar("SELECT error_summary FROM check_runs WHERE id = ?")
            .bind(result.run_id.as_deref().unwrap())
            .fetch_one(&fixture.repo.pool)
            .await
            .unwrap();
    assert_eq!(persisted_error, "approved asset record is missing owner");
}

#[tokio::test]
async fn approved_asset_without_a_type_fails_locally() {
    let fixture = runner_fixture(Role::Operator).await;
    sqlx::query("UPDATE assets SET kind = '' WHERE id = ?")
        .bind(&fixture.asset.id)
        .execute(&fixture.repo.pool)
        .await
        .unwrap();

    let result = Runner::new(fixture.repo, fixture.user)
        .start(fixture.asset, "asset-record-review", asset_record_input())
        .await
        .unwrap();

    assert_eq!(result.state, RunState::Failed);
    assert_eq!(
        result.error.as_deref(),
        Some("approved asset record is missing type")
    );
}

#[tokio::test]
async fn approved_asset_without_an_address_fails_locally() {
    let fixture = runner_fixture(Role::Operator).await;
    sqlx::query("UPDATE assets SET address = '\t' WHERE id = ?")
        .bind(&fixture.asset.id)
        .execute(&fixture.repo.pool)
        .await
        .unwrap();

    let result = Runner::new(fixture.repo, fixture.user)
        .start(fixture.asset, "asset-record-review", asset_record_input())
        .await
        .unwrap();

    assert_eq!(result.state, RunState::Failed);
    assert_eq!(
        result.error.as_deref(),
        Some("approved asset record is missing address")
    );
}

#[tokio::test]
async fn unauthorized_runner_does_not_create_a_run() {
    let fixture = runner_fixture(Role::Viewer).await;
    let result = Runner::new(fixture.repo.clone(), fixture.user)
        .start(fixture.asset, "asset-record-review", asset_record_input())
        .await;

    assert!(matches!(result, Err(AppError::Forbidden)));
    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM check_runs")
        .fetch_one(&fixture.repo.pool)
        .await
        .unwrap();
    assert_eq!(run_count, 0);
}

#[tokio::test]
async fn cancellation_is_persisted_and_never_reported_as_success() {
    let fixture = runner_fixture(Role::Operator).await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let result = Runner::with_cancellation(fixture.repo.clone(), fixture.user, cancellation)
        .start(fixture.asset, "asset-record-review", asset_record_input())
        .await
        .unwrap();

    assert_eq!(result.state, RunState::Cancelled);
    let persisted_state: RunState = sqlx::query_scalar("SELECT state FROM check_runs WHERE id = ?")
        .bind(result.run_id.as_deref().unwrap())
        .fetch_one(&fixture.repo.pool)
        .await
        .unwrap();
    assert_eq!(persisted_state, RunState::Cancelled);
}

#[tokio::test]
async fn asset_values_cannot_make_persisted_output_exceed_the_catalog_limit() {
    let fixture = runner_fixture(Role::Operator).await;
    sqlx::query("UPDATE assets SET owner = ? WHERE id = ?")
        .bind("x".repeat(20_000))
        .bind(&fixture.asset.id)
        .execute(&fixture.repo.pool)
        .await
        .unwrap();
    let result = Runner::new(fixture.repo, fixture.user)
        .start(fixture.asset, "asset-record-review", asset_record_input())
        .await
        .unwrap();

    let output = serde_json::to_vec(&result).unwrap();
    assert!(output.len() <= catalog()[0].max_output_bytes);
}
