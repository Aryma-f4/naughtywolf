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

#[tokio::test]
async fn scoped_portal_reads_exclude_records_without_membership() {
    let repo = test_repository().await;
    let allowed = repo
        .create_operation("Allowed", "Portal fixture")
        .await
        .unwrap();
    let hidden = repo
        .create_operation("Hidden", "Portal fixture")
        .await
        .unwrap();
    let allowed_asset = repo
        .create_asset(&allowed.id, "allowed-host", "host", "lab", "10.0.0.1")
        .await
        .unwrap();
    let hidden_asset = repo
        .create_asset(&hidden.id, "hidden-host", "host", "lab", "10.0.0.2")
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind("operator")
        .bind("operator")
        .bind("not-a-real-password-hash")
        .bind("operator")
        .execute(&repo.pool)
        .await
        .unwrap();
    repo.add_member(&allowed.id, "operator").await.unwrap();
    repo.ensure_builtin_check("portal-test", "Portal test", "operator", 30, 4_096)
        .await
        .unwrap();
    let allowed_run = repo
        .create_run(
            "portal-test",
            &allowed_asset.id,
            &allowed.id,
            Some("operator"),
            &json!({}),
        )
        .await
        .unwrap();
    let hidden_run = repo
        .create_run(
            "portal-test",
            &hidden_asset.id,
            &hidden.id,
            Some("operator"),
            &json!({}),
        )
        .await
        .unwrap();
    let allowed_evidence = repo
        .create_evidence(
            &allowed_run.id,
            "allowed.json",
            "application/json",
            0,
            "allowed",
        )
        .await
        .unwrap();
    let hidden_evidence = repo
        .create_evidence(
            &hidden_run.id,
            "hidden.json",
            "application/json",
            0,
            "hidden",
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO audit_events \
         (id, actor_id, operation_id, action, target_type, target_id, parameter_summary, outcome, correlation_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("allowed-audit")
    .bind("operator")
    .bind(&allowed.id)
    .bind("operation.viewed")
    .bind("operation")
    .bind(&allowed.id)
    .bind(Option::<&str>::None)
    .bind("success")
    .bind("allowed-correlation")
    .execute(&repo.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO audit_events \
         (id, actor_id, operation_id, action, target_type, target_id, parameter_summary, outcome, correlation_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("hidden-audit")
    .bind("operator")
    .bind(&hidden.id)
    .bind("operation.viewed")
    .bind("operation")
    .bind(&hidden.id)
    .bind(Option::<&str>::None)
    .bind("success")
    .bind("hidden-correlation")
    .execute(&repo.pool)
    .await
    .unwrap();

    assert_eq!(
        repo.list_operations_visible_to("operator", false)
            .await
            .unwrap()
            .iter()
            .map(|item| &item.id)
            .collect::<Vec<_>>(),
        vec![&allowed.id]
    );
    assert_eq!(
        repo.list_assets_visible_to("operator", false)
            .await
            .unwrap()
            .iter()
            .map(|item| &item.id)
            .collect::<Vec<_>>(),
        vec![&allowed_asset.id]
    );
    assert_eq!(
        repo.list_check_runs_visible_to("operator", false)
            .await
            .unwrap()
            .iter()
            .map(|item| &item.id)
            .collect::<Vec<_>>(),
        vec![&allowed_run.id]
    );
    assert_eq!(
        repo.list_evidence_visible_to("operator", false)
            .await
            .unwrap()
            .iter()
            .map(|item| &item.id)
            .collect::<Vec<_>>(),
        vec![&allowed_evidence.id]
    );
    assert_eq!(
        repo.list_audit_events_visible_to("operator", false)
            .await
            .unwrap()
            .iter()
            .map(|item| &item.id)
            .collect::<Vec<_>>(),
        vec!["allowed-audit"]
    );
    assert_eq!(
        repo.count_operations_visible_to("operator", false)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.count_assets_visible_to("operator", false)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.count_check_runs_visible_to("operator", false)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.count_evidence_visible_to("operator", false)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.count_audit_events_visible_to("operator", false)
            .await
            .unwrap(),
        1
    );

    assert_ne!(allowed_evidence.id, hidden_evidence.id);
}

#[tokio::test]
async fn admin_scoped_portal_counts_include_all_operations() {
    let repo = test_repository().await;
    let first = repo
        .create_operation("First", "Portal fixture")
        .await
        .unwrap();
    let second = repo
        .create_operation("Second", "Portal fixture")
        .await
        .unwrap();
    let first_asset = repo
        .create_asset(&first.id, "first-host", "host", "lab", "10.0.0.1")
        .await
        .unwrap();
    let second_asset = repo
        .create_asset(&second.id, "second-host", "host", "lab", "10.0.0.2")
        .await
        .unwrap();
    repo.ensure_builtin_check(
        "portal-admin-test",
        "Portal admin test",
        "operator",
        30,
        4_096,
    )
    .await
    .unwrap();
    let first_run = repo
        .create_run(
            "portal-admin-test",
            &first_asset.id,
            &first.id,
            None,
            &json!({}),
        )
        .await
        .unwrap();
    let second_run = repo
        .create_run(
            "portal-admin-test",
            &second_asset.id,
            &second.id,
            None,
            &json!({}),
        )
        .await
        .unwrap();
    repo.create_evidence(&first_run.id, "first.json", "application/json", 0, "first")
        .await
        .unwrap();
    repo.create_evidence(
        &second_run.id,
        "second.json",
        "application/json",
        0,
        "second",
    )
    .await
    .unwrap();

    assert_eq!(
        repo.count_operations_visible_to("admin", true)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        repo.count_assets_visible_to("admin", true).await.unwrap(),
        2
    );
    assert_eq!(
        repo.count_check_runs_visible_to("admin", true)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        repo.count_evidence_visible_to("admin", true).await.unwrap(),
        2
    );
    assert_eq!(
        repo.count_audit_events_visible_to("admin", true)
            .await
            .unwrap(),
        0
    );
}
