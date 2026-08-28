use naughtywolf::{
    AppError,
    auth::{AuthenticatedUser, rbac::Role},
    db::{self, repositories::Repository},
    policy::{authorize_asset_run, authorize_operation},
};

struct PolicyFixture {
    repo: Repository,
    operator: AuthenticatedUser,
    viewer: AuthenticatedUser,
    admin: AuthenticatedUser,
    asset: naughtywolf::db::models::Asset,
    foreign_asset: naughtywolf::db::models::Asset,
}

async fn policy_fixture() -> PolicyFixture {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repo = Repository { pool };

    let operation = repo
        .create_operation("Authorized operation", "Policy test fixture")
        .await
        .unwrap();
    let foreign_operation = repo
        .create_operation("Foreign operation", "Policy test fixture")
        .await
        .unwrap();
    sqlx::query("UPDATE operations SET status = 'active'")
        .execute(&repo.pool)
        .await
        .unwrap();

    let operator = insert_user(&repo, "operator", Role::Operator).await;
    let viewer = insert_user(&repo, "viewer", Role::Viewer).await;
    let admin = insert_user(&repo, "admin", Role::Admin).await;
    repo.add_member(&operation.id, &operator.id).await.unwrap();
    repo.add_member(&operation.id, &viewer.id).await.unwrap();

    let asset = repo
        .create_asset(&operation.id, "owned-asset", "host", "lab", "10.0.0.8")
        .await
        .unwrap();
    let foreign_asset = repo
        .create_asset(
            &foreign_operation.id,
            "foreign-asset",
            "host",
            "lab",
            "10.0.0.9",
        )
        .await
        .unwrap();

    PolicyFixture {
        repo,
        operator,
        viewer,
        admin,
        asset,
        foreign_asset,
    }
}

async fn insert_user(repo: &Repository, username: &str, role: Role) -> AuthenticatedUser {
    let id = format!("{username}-id");
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(username)
        .bind("not-a-real-password-hash")
        .bind(role.to_string())
        .execute(&repo.pool)
        .await
        .unwrap();

    AuthenticatedUser {
        id,
        username: username.to_owned(),
        role,
    }
}

#[tokio::test]
async fn operator_can_run_check_within_membership() {
    let fixture = policy_fixture().await;

    assert!(
        authorize_asset_run(&fixture.repo, &fixture.operator, &fixture.asset.id)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn operator_cannot_run_check_outside_membership() {
    let fixture = policy_fixture().await;

    let result =
        authorize_asset_run(&fixture.repo, &fixture.operator, &fixture.foreign_asset.id).await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

#[tokio::test]
async fn viewer_never_satisfies_operator_requirement() {
    let fixture = policy_fixture().await;

    let result = authorize_asset_run(&fixture.repo, &fixture.viewer, &fixture.asset.id).await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

#[tokio::test]
async fn admin_bypasses_operation_membership() {
    let fixture = policy_fixture().await;

    assert!(
        authorize_operation(
            &fixture.repo,
            &fixture.admin,
            &fixture.foreign_asset.operation_id,
            Role::Operator,
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn closed_operation_rejects_new_runs_even_for_admin() {
    let fixture = policy_fixture().await;
    sqlx::query("UPDATE operations SET status = 'closed' WHERE id = ?")
        .bind(&fixture.asset.operation_id)
        .execute(&fixture.repo.pool)
        .await
        .unwrap();

    let result = authorize_asset_run(&fixture.repo, &fixture.admin, &fixture.asset.id).await;

    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn retired_asset_rejects_new_runs_even_for_admin() {
    let fixture = policy_fixture().await;
    sqlx::query("UPDATE assets SET status = 'retired' WHERE id = ?")
        .bind(&fixture.asset.id)
        .execute(&fixture.repo.pool)
        .await
        .unwrap();

    let result = authorize_asset_run(&fixture.repo, &fixture.admin, &fixture.asset.id).await;

    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn unknown_asset_is_not_found() {
    let fixture = policy_fixture().await;

    let result = authorize_asset_run(&fixture.repo, &fixture.admin, "missing-asset").await;

    assert!(matches!(result, Err(AppError::NotFound)));
}
