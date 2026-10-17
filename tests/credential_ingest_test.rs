//! Credential ingestion through the portal's own C2 path, and the operation
//! scoping that decides who may read recovered secrets.
//!
//! The portal binary mounts its own C2 handler rather than the standalone
//! `nw-server`, so credentials harvested by a beacon checking in against the
//! web GUI were previously discarded: only `nw-server` knew how to persist
//! them. These tests cover that path end to end.

use naughtywolf::{
    auth::rbac::Role,
    db::{self, repositories::Repository},
};

async fn repository() -> Repository {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    Repository { pool }
}

async fn add_callback(repo: &Repository, id: &str, operation_id: Option<&str>, host: &str) {
    sqlx::query(
        "INSERT INTO callbacks (id, host, user_name, process, arch, os, protocol, status) \
         VALUES (?1, ?2, 'u', 'p', 'x86_64', 'linux', 'http', 'active')",
    )
    .bind(id)
    .bind(host)
    .execute(&repo.pool)
    .await
    .unwrap();
    if let Some(op) = operation_id {
        sqlx::query("UPDATE callbacks SET operation_id = ?1 WHERE id = ?2")
            .bind(op)
            .bind(id)
            .execute(&repo.pool)
            .await
            .unwrap();
    }
}

async fn add_operation_with_member(repo: &Repository, op: &str, user: &str) {
    sqlx::query("INSERT OR IGNORE INTO operations (id, name, purpose) VALUES (?1, ?1, 'test')")
        .bind(op)
        .execute(&repo.pool)
        .await
        .unwrap();
    sqlx::query("INSERT OR IGNORE INTO users (id, username, password_hash, role) VALUES (?1, ?1, 'x', 'operator')")
        .bind(user)
        .execute(&repo.pool)
        .await
        .unwrap();
    sqlx::query("INSERT OR IGNORE INTO operation_members (operation_id, user_id) VALUES (?1, ?2)")
        .bind(op)
        .bind(user)
        .execute(&repo.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn harvested_credentials_round_trip_through_the_store() {
    let repo = repository().await;

    repo.upsert_credential("sess-1", "shadow", "hash", "root:$6$abc", "secret-value")
        .await
        .unwrap();

    let rows = repo
        .list_credentials_visible_to("admin-1", true)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "one credential persisted");
    let cred = &rows[0];
    assert_eq!(cred.cred_type, "hash");
    assert_eq!(cred.source, "shadow");
    assert_eq!(cred.identifier, "root:$6$abc");
    assert_eq!(cred.secret, "secret-value");
    assert_eq!(cred.session_id, "sess-1");
}

#[tokio::test]
async fn re_harvesting_the_same_credential_updates_in_place() {
    let repo = repository().await;

    repo.upsert_credential("sess-1", "shadow", "hash", "root:$6$abc", "old-secret")
        .await
        .unwrap();
    repo.upsert_credential("sess-1", "shadow", "hash", "root:$6$abc", "rotated-secret")
        .await
        .unwrap();

    let rows = repo
        .list_credentials_visible_to("admin-1", true)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "re-harvest must not duplicate the row");
    assert_eq!(rows[0].secret, "rotated-secret");

    // Distinct identifiers still coexist.
    repo.upsert_credential("sess-1", "shadow", "hash", "daemon:x", "another")
        .await
        .unwrap();
    let rows = repo
        .list_credentials_visible_to("admin-1", true)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "a different identifier is a different entry");
}

#[tokio::test]
async fn callback_host_is_resolved_for_display() {
    let repo = repository().await;
    add_callback(&repo, "sess-1", None, "web-01").await;

    repo.upsert_credential("sess-1", "shadow", "hash", "root:abc", "s")
        .await
        .unwrap();

    let rows = repo
        .list_credentials_visible_to("admin-1", true)
        .await
        .unwrap();
    assert_eq!(rows[0].callback_host.as_deref(), Some("web-01"));
}

/// The security-critical case: a credential harvested on a callback inside
/// another operator's operation must not be readable by this operator.
#[tokio::test]
async fn credentials_are_scoped_to_operations_like_their_callback() {
    let repo = repository().await;
    add_operation_with_member(&repo, "op-mine", "user-a").await;
    add_operation_with_member(&repo, "op-theirs", "user-b").await;
    add_callback(&repo, "sess-mine", Some("op-mine"), "host-a").await;
    add_callback(&repo, "sess-theirs", Some("op-theirs"), "host-b").await;

    repo.upsert_credential("sess-mine", "shadow", "hash", "mine:abc", "mine-secret")
        .await
        .unwrap();
    repo.upsert_credential(
        "sess-theirs",
        "shadow",
        "hash",
        "theirs:abc",
        "theirs-secret",
    )
    .await
    .unwrap();

    let mine = repo
        .list_credentials_visible_to("user-a", false)
        .await
        .unwrap();
    assert_eq!(mine.len(), 1, "only this operator's operation is visible");
    assert_eq!(mine[0].identifier, "mine:abc");

    // Admin sees both.
    let all = repo
        .list_credentials_visible_to("admin-1", true)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
}

/// A callback with no operation has no membership to join through. Treating
/// "unassigned" as visible to everyone would leak harvested secrets, so it
/// stays admin-only.
#[tokio::test]
async fn unassigned_callback_credentials_are_admin_only() {
    let repo = repository().await;
    add_operation_with_member(&repo, "op-mine", "user-a").await;
    add_callback(&repo, "sess-orphan", None, "host-orphan").await;
    add_callback(&repo, "sess-mine", Some("op-mine"), "host-a").await;

    repo.upsert_credential(
        "sess-orphan",
        "shadow",
        "hash",
        "orphan:abc",
        "orphan-secret",
    )
    .await
    .unwrap();
    repo.upsert_credential("sess-mine", "shadow", "hash", "mine:abc", "mine-secret")
        .await
        .unwrap();

    let operator = repo
        .list_credentials_visible_to("user-a", false)
        .await
        .unwrap();
    assert_eq!(
        operator.len(),
        1,
        "an unassigned callback must not leak to a plain operator"
    );
    assert_eq!(operator[0].identifier, "mine:abc");

    let admin = repo
        .list_credentials_visible_to("admin-1", true)
        .await
        .unwrap();
    assert_eq!(admin.len(), 2, "admin still sees both");
}

#[tokio::test]
async fn looking_up_an_invisible_credential_is_not_found() {
    let repo = repository().await;
    add_operation_with_member(&repo, "op-theirs", "user-b").await;
    add_callback(&repo, "sess-theirs", Some("op-theirs"), "host-b").await;
    repo.upsert_credential(
        "sess-theirs",
        "shadow",
        "hash",
        "theirs:abc",
        "theirs-secret",
    )
    .await
    .unwrap();

    let id = Repository::credential_entry_id("sess-theirs", "shadow", "hash", "theirs:abc");

    // The owner can read it.
    let found = repo
        .find_credential_visible_to(&id, "user-b", false)
        .await
        .expect("member of the owning operation can read it");
    assert_eq!(found.identifier, "theirs:abc");

    // Anyone else gets NotFound rather than the row.
    let err = repo
        .find_credential_visible_to(&id, "user-a", false)
        .await
        .expect_err("a non-member must not read the credential");
    assert!(matches!(err, naughtywolf::AppError::NotFound), "{err:?}");
}

/// The entry id must match the standalone C2 server's scheme, or the two
/// processes would write duplicate rows for the same harvested credential.
#[test]
fn entry_id_matches_the_standalone_server_scheme() {
    assert_eq!(
        Repository::credential_entry_id("s1", "shadow", "hash", "root:abc"),
        "s1:shadow:hash:root:abc"
    );
    assert_eq!(
        nw_server::creds::CredentialStore::entry_id("s1", "shadow", "hash", "root:abc"),
        "s1:shadow:hash:root:abc"
    );
}

#[tokio::test]
async fn the_credentials_page_escapes_untrusted_values() {
    use naughtywolf::db::models::HarvestedCredential;

    let row = HarvestedCredential {
        id: "x".into(),
        session_id: "s1".into(),
        source: "shadow".into(),
        cred_type: "hash".into(),
        identifier: "<script>alert(1)</script>".into(),
        secret: "\"onload=\"alert(2)".into(),
        created_at: "now".into(),
        callback_host: None,
    };
    let viewer = naughtywolf::auth::AuthenticatedUser {
        id: "operator-1".into(),
        username: "operator-1".into(),
        role: Role::Operator,
    };
    let html = naughtywolf::portal::templates::credentials_page(&viewer, &[row]);

    assert!(
        !html.contains("<script>"),
        "raw script tag must not survive"
    );
    assert!(
        !html.contains("onload=\"alert"),
        "attribute break-out must not survive"
    );
    assert!(
        html.contains("&lt;script&gt;"),
        "value is escaped, not dropped"
    );
}
