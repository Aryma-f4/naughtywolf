use naughtywolf::db::models::CheckRunState;
use naughtywolf::db::repositories::Repository;
use serde_json::json;

async fn test_repository() -> Repository {
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();
    Repository { pool }
}

#[tokio::test]
async fn asset_is_scoped_to_its_operation() {
    let repo = test_repository().await;
    let operation = repo
        .create_operation("Linux lab", "Course exercise")
        .await
        .unwrap();
    let asset = repo
        .create_asset(&operation.id, "web-01", "host", "lab", "10.0.0.8")
        .await
        .unwrap();

    assert_eq!(asset.operation_id, operation.id);
    assert_eq!(repo.list_assets(&operation.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn repository_rejects_asset_for_unknown_operation() {
    let repo = test_repository().await;

    assert!(
        repo.create_asset("missing", "x", "host", "lab", "127.0.0.1")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn member_is_persisted_for_its_operation() {
    let repo = test_repository().await;
    let operation = repo
        .create_operation("Linux lab", "Course exercise")
        .await
        .unwrap();
    let user_id = "user-1";
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind(user_id)
        .bind("lab-admin")
        .bind("not-a-real-password-hash")
        .bind("admin")
        .execute(&repo.pool)
        .await
        .unwrap();

    repo.add_member(&operation.id, user_id).await.unwrap();

    let member_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM operation_members WHERE operation_id = ? AND user_id = ?",
    )
    .bind(&operation.id)
    .bind(user_id)
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    assert_eq!(member_count, 1);
}

#[tokio::test]
async fn check_run_is_queued_then_finished_with_its_result() {
    let repo = test_repository().await;
    let operation = repo
        .create_operation("Linux lab", "Course exercise")
        .await
        .unwrap();
    let asset = repo
        .create_asset(&operation.id, "web-01", "host", "lab", "10.0.0.8")
        .await
        .unwrap();
    let user_id = "user-1";
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind(user_id)
        .bind("lab-admin")
        .bind("not-a-real-password-hash")
        .bind("admin")
        .execute(&repo.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO checks \
         (id, name, description, version, required_role, input_schema, result_schema, timeout_seconds, output_limit_bytes) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("asset-record-review")
    .bind("Asset record review")
    .bind("Verifies the approved asset record")
    .bind("1")
    .bind("operator")
    .bind("{}")
    .bind("{}")
    .bind(30_i64)
    .bind(4_096_i64)
    .execute(&repo.pool)
    .await
    .unwrap();

    let run = repo
        .create_run(
            "asset-record-review",
            &asset.id,
            &operation.id,
            Some(user_id),
            &json!({}),
        )
        .await
        .unwrap();
    assert_eq!(run.state, CheckRunState::Queued);

    let finished = repo
        .finish_run(
            &run.id,
            CheckRunState::Succeeded,
            Some(&json!({"finding": "asset record complete"})),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(finished.state, CheckRunState::Succeeded);
    assert_eq!(
        finished.result_json,
        Some(json!({"finding": "asset record complete"}))
    );
    assert!(finished.finished_at.is_some());
}
