use naughtywolf::{
    audit::AuditEntry,
    db::{self, repositories::Repository},
    evidence::EvidenceStore,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

async fn test_repository() -> Repository {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    Repository { pool }
}

async fn test_store() -> (EvidenceStore, Repository, TempDir, String) {
    let repo = test_repository().await;
    let operation = repo
        .create_operation("Evidence lab", "Evidence fixture")
        .await
        .unwrap();
    let asset = repo
        .create_asset(&operation.id, "web-01", "host", "lab", "127.0.0.1")
        .await
        .unwrap();
    repo.ensure_builtin_check("evidence-test", "Evidence test", "operator", 30, 7)
        .await
        .unwrap();
    let run = repo
        .create_run(
            "evidence-test",
            &asset.id,
            &operation.id,
            None,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    let directory = TempDir::new().unwrap();
    let store = EvidenceStore::new(repo.clone(), directory.path());

    (store, repo, directory, run.id)
}

#[tokio::test]
async fn evidence_stores_sha256_metadata_and_a_generated_path() {
    let (store, repo, directory, run_id) = test_store().await;

    let evidence = store
        .write(&run_id, b"finding", "application/json")
        .await
        .unwrap();

    assert_eq!(
        evidence.sha256,
        "8bf18252cb6dadd5f42fca108271e688696209cdb6fae22e32318b204e6eb37d"
    );
    assert_eq!(evidence.byte_len, 7);
    assert_eq!(evidence.check_run_id, run_id);
    assert!(evidence.storage_path.ends_with(".json"));
    assert_eq!(
        tokio::fs::read(directory.path().join(&evidence.storage_path))
            .await
            .unwrap(),
        b"finding"
    );
    assert_eq!(repo.count_evidence().await.unwrap(), 1);
}

#[tokio::test]
async fn evidence_rejects_bytes_larger_than_the_run_catalog_limit_before_writing() {
    let (store, repo, directory, run_id) = test_store().await;

    let result = store.write(&run_id, b"findings", "application/json").await;

    assert!(result.is_err());
    assert_eq!(repo.count_evidence().await.unwrap(), 0);
    assert!(!directory.path().join(run_id).exists());
}

#[tokio::test]
async fn evidence_rejects_a_preexisting_generated_output_file() {
    let (store, repo, directory, run_id) = test_store().await;
    let digest = hex::encode(Sha256::digest(b"finding"));
    let output_path = directory
        .path()
        .join(&run_id)
        .join(format!("{}.json", &digest[..16]));
    tokio::fs::create_dir_all(output_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&output_path, b"attacker-controlled bytes")
        .await
        .unwrap();

    let result = store.write(&run_id, b"finding", "application/json").await;

    assert!(result.is_err());
    assert_eq!(
        tokio::fs::read(output_path).await.unwrap(),
        b"attacker-controlled bytes"
    );
    assert_eq!(repo.count_evidence().await.unwrap(), 0);
}

#[tokio::test]
async fn evidence_rejects_a_symlinked_run_directory() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let (store, repo, directory, run_id) = test_store().await;
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), directory.path().join(&run_id)).unwrap();

        let result = store.write(&run_id, b"finding", "application/json").await;

        assert!(result.is_err());
        assert_eq!(repo.count_evidence().await.unwrap(), 0);
        assert!(
            std::fs::read_dir(outside.path()).unwrap().next().is_none(),
            "evidence write escaped into the symlink target"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cached_evidence_is_rejected_when_its_run_directory_becomes_a_symlink() {
    use std::os::unix::fs::symlink;

    let (store, repo, directory, run_id) = test_store().await;
    store
        .write(&run_id, b"finding", "application/json")
        .await
        .unwrap();
    let run_directory = directory.path().join(&run_id);
    let outside = TempDir::new().unwrap();
    std::fs::remove_dir_all(&run_directory).unwrap();
    symlink(outside.path(), &run_directory).unwrap();

    let result = store.write(&run_id, b"finding", "application/json").await;

    assert!(result.is_err());
    assert_eq!(repo.count_evidence().await.unwrap(), 1);
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn cached_evidence_is_rejected_when_its_output_becomes_a_symlink() {
    use std::os::unix::fs::symlink;

    let (store, repo, directory, run_id) = test_store().await;
    let evidence = store
        .write(&run_id, b"finding", "application/json")
        .await
        .unwrap();
    let output_path = directory.path().join(&evidence.storage_path);
    let outside = TempDir::new().unwrap();
    std::fs::remove_file(&output_path).unwrap();
    symlink(outside.path().join("replacement"), &output_path).unwrap();

    let result = store.write(&run_id, b"finding", "application/json").await;

    assert!(result.is_err());
    assert_eq!(repo.count_evidence().await.unwrap(), 1);
    assert!(!outside.path().join("replacement").exists());
}

#[tokio::test]
async fn cached_evidence_is_rejected_when_its_regular_file_is_replaced() {
    let (store, _repo, directory, run_id) = test_store().await;
    let evidence = store
        .write(&run_id, b"finding", "application/json")
        .await
        .unwrap();
    let output_path = directory.path().join(&evidence.storage_path);
    tokio::fs::write(&output_path, b"changed").await.unwrap();

    let result = store.write(&run_id, b"finding", "application/json").await;

    assert!(result.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn evidence_root_is_owner_only_after_creation() {
    use std::os::unix::fs::PermissionsExt;

    let (_store, repo, directory, run_id) = test_store().await;
    let evidence_root = directory.path().join("generated-evidence-root");
    let store = EvidenceStore::new(repo, &evidence_root);

    store
        .write(&run_id, b"finding", "application/json")
        .await
        .unwrap();

    assert_eq!(
        std::fs::metadata(evidence_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[tokio::test]
async fn operation_update_and_audit_are_atomic() {
    let repo = test_repository().await;

    assert!(
        repo.rename_operation_with_audit("missing", "new", "actor", "cid")
            .await
            .is_err()
    );
    assert_eq!(repo.count_audit_events().await.unwrap(), 0);
}

#[tokio::test]
async fn failed_audit_insert_rolls_back_the_operation_rename() {
    let repo = test_repository().await;
    let operation = repo
        .create_operation("Original", "Audit rollback fixture")
        .await
        .unwrap();

    let result = repo
        .rename_operation_with_audit(&operation.id, "Renamed", "missing-actor", "cid")
        .await;

    assert!(result.is_err());
    assert_eq!(
        repo.find_operation(&operation.id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Original"
    );
    assert_eq!(repo.count_audit_events().await.unwrap(), 0);
}

#[tokio::test]
async fn operation_rename_persists_its_append_only_audit_event() {
    let repo = test_repository().await;
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind("actor")
        .bind("actor")
        .bind("not-a-real-password-hash")
        .bind("admin")
        .execute(&repo.pool)
        .await
        .unwrap();
    let operation = repo
        .create_operation("Original", "Audit fixture")
        .await
        .unwrap();
    let renamed = repo
        .rename_operation_with_audit(&operation.id, "Renamed", "actor", "cid")
        .await
        .unwrap();

    assert_eq!(renamed.name, "Renamed");
    assert_eq!(repo.count_audit_events().await.unwrap(), 1);
}

#[test]
fn audit_entry_captures_required_append_only_fields() {
    let entry = AuditEntry::new(
        "actor",
        "operation.renamed",
        "operation",
        "operation-1",
        "success",
        "cid",
    );

    assert_eq!(entry.actor_id.as_deref(), Some("actor"));
    assert_eq!(entry.target_id.as_deref(), Some("operation-1"));
    assert_eq!(entry.correlation_id, "cid");
}

#[tokio::test]
async fn catalog_output_limits_smaller_than_json_fallback_are_rejected() {
    let repo = test_repository().await;

    let result = repo
        .ensure_builtin_check("too-small", "Too small", "operator", 30, 1)
        .await;

    assert!(result.is_err());
}
