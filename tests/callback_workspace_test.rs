use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Extension,
    http::{Request, StatusCode},
    response::Response,
    routing::post,
};
use futures::StreamExt;
use naughtywolf::{
    auth::{AuthenticatedUser, middleware::AuthSession, rbac::Role},
    callback_workspace::transfers::{TransferError, TransferStore},
    db::{self, repositories::Repository},
    portal,
};
use nw_profile::msgs::{FileAck, FileChunk};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};

async fn test_repository() -> Repository {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    Repository { pool }
}

async fn create_user(repo: &Repository, id: &str, role: Role) -> AuthenticatedUser {
    let username = format!("{id}-user");
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(&username)
        .bind("test-password-hash")
        .bind(role.to_string())
        .execute(&repo.pool)
        .await
        .unwrap();
    AuthenticatedUser {
        id: id.to_owned(),
        username,
        role,
    }
}

async fn create_scoped_callback(repo: &Repository, user_id: &str, session_id: &str) {
    let operation = repo
        .create_operation("Task API Lab", "Pagination tests")
        .await
        .unwrap();
    sqlx::query("INSERT INTO operation_members (operation_id, user_id) VALUES (?, ?)")
        .bind(&operation.id)
        .bind(user_id)
        .execute(&repo.pool)
        .await
        .unwrap();
    repo.upsert_callback(
        session_id,
        "task-api-host",
        "operator",
        "linux",
        "x86_64",
        42,
        "test-session-key",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE callbacks SET operation_id = ? WHERE id = ?")
        .bind(&operation.id)
        .bind(session_id)
        .execute(&repo.pool)
        .await
        .unwrap();
}

async fn transfer_fixture(
    max_bytes: u64,
) -> (
    tempfile::TempDir,
    Repository,
    TransferStore,
    AuthenticatedUser,
    String,
) {
    let directory = tempfile::tempdir().unwrap();
    let repo = test_repository().await;
    let operator = create_user(
        &repo,
        &format!("transfer-{}", uuid::Uuid::new_v4()),
        Role::Operator,
    )
    .await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let store = TransferStore::new(repo.clone(), directory.path(), max_bytes).unwrap();
    (directory, repo, store, operator, session_id)
}

fn transfer_chunk(
    transfer: &naughtywolf::db::models::FileTransfer,
    offset: u64,
    total: u64,
    data: &[u8],
) -> FileChunk {
    FileChunk {
        transfer_id: Some(uuid::Uuid::parse_str(&transfer.id).unwrap()),
        task_id: transfer
            .task_id
            .as_deref()
            .map(|id| uuid::Uuid::parse_str(id).unwrap()),
        name: "untrusted/../../remote-name.bin".into(),
        offset,
        total,
        data: data.to_vec(),
    }
}

#[tokio::test]
async fn transfer_store_persists_contiguous_progress_and_resumes_after_reopen() {
    let (directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let expected = hex::encode(Sha256::digest(b"abcdef"));
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/remote.bin",
            Some(6),
            Some(&expected),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();

    let first = store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 6, b"abc"))
        .await
        .unwrap();
    assert_eq!(first.received, 3);
    assert!(!first.done);

    let reopened = TransferStore::new(repo.clone(), directory.path(), 4096).unwrap();
    let second = reopened
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 3, 6, b"def"))
        .await
        .unwrap();
    assert_eq!(second.received, 6);
    assert!(second.done);
    let repeated_final_ack = reopened
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 3, 6, b"def"))
        .await
        .unwrap();
    assert!(
        repeated_final_ack.done,
        "a lost completion ACK must be repeatable"
    );
    let provisional = reopened.transfer(&transfer.id).await.unwrap().unwrap();
    assert_eq!(provisional.status, "active");
    repo.store_task_result_for_session(
        &session_id,
        transfer.task_id.as_deref().unwrap(),
        true,
        b"download confirmed",
        b"",
        0,
    )
    .await
    .unwrap();
    let finished = reopened.transfer(&transfer.id).await.unwrap().unwrap();
    assert_eq!(finished.status, "completed");
    assert_eq!(reopened.read_completed(&finished).await.unwrap(), b"abcdef");
    assert!(
        !directory
            .path()
            .join("callback-transfers")
            .join(format!("{}.part", transfer.storage_key))
            .exists()
    );
    assert!(
        directory
            .path()
            .join("callback-transfers")
            .join(&transfer.storage_key)
            .is_file()
    );
}

#[tokio::test]
async fn transfer_store_treats_exact_duplicates_idempotently_and_rejects_gaps() {
    let (_directory, _repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/remote.bin",
            Some(6),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 6, b"abc"))
        .await
        .unwrap();
    let duplicate = store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 6, b"abc"))
        .await
        .unwrap();
    assert_eq!(duplicate.received, 3);
    let gap = store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 4, 6, b"ef"))
        .await
        .unwrap_err();
    assert!(matches!(
        gap,
        TransferError::OffsetGap {
            expected: 3,
            actual: 4
        }
    ));
}

#[tokio::test]
async fn transfer_store_rejects_missing_or_mismatched_protocol_ids_stably() {
    let (_directory, _repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/remote.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let mut chunk = transfer_chunk(&transfer, 0, 3, b"abc");
    chunk.transfer_id = None;
    assert_eq!(
        store
            .receive_chunk(&session_id, &chunk)
            .await
            .unwrap_err()
            .to_string(),
        "transfer protocol id is required"
    );
    chunk.transfer_id = Some(uuid::Uuid::new_v4());
    assert_eq!(
        store
            .receive_chunk(&session_id, &chunk)
            .await
            .unwrap_err()
            .to_string(),
        "transfer protocol id does not match an authorized transfer"
    );
}

#[tokio::test]
async fn transfer_store_enforces_size_limit_and_checksum_before_publication() {
    let (directory, _repo, store, operator, session_id) = transfer_fixture(5).await;
    let oversized = store
        .queue_download(
            &session_id,
            "/srv/large.bin",
            Some(6),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap_err();
    assert!(matches!(oversized, TransferError::SizeLimit { .. }));

    let transfer = store
        .queue_download(
            &session_id,
            "/srv/bad.bin",
            Some(3),
            Some(&"0".repeat(64)),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let error = store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 3, b"abc"))
        .await
        .unwrap_err();
    assert!(matches!(error, TransferError::ChecksumMismatch));
    assert!(
        !directory
            .path()
            .join("callback-transfers")
            .join(&transfer.storage_key)
            .exists()
    );
    let failed = store.transfer(&transfer.id).await.unwrap().unwrap();
    assert_eq!(failed.status, "error");
}

#[tokio::test]
async fn transfer_store_refuses_unverified_or_failed_downloads() {
    let (_directory, _repo, store, operator, session_id) = transfer_fixture(4096).await;
    let active = store
        .queue_download(
            &session_id,
            "/srv/active.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    assert!(matches!(
        store.read_completed(&active).await.unwrap_err(),
        TransferError::NotDownloadable
    ));
    let failed = store
        .queue_download(
            &session_id,
            "/srv/failed.bin",
            Some(3),
            Some(&"0".repeat(64)),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let _ = store
        .receive_chunk(&session_id, &transfer_chunk(&failed, 0, 3, b"abc"))
        .await;
    let failed = store.transfer(&failed.id).await.unwrap().unwrap();
    assert!(matches!(
        store.read_completed(&failed).await.unwrap_err(),
        TransferError::NotDownloadable
    ));
}

#[tokio::test]
async fn transfer_store_upload_chunks_resume_from_durable_acks_with_fifo_fairness() {
    let (directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let mut first_stage = store.begin_upload_stage().unwrap();
    first_stage.write(b"abcdef").unwrap();
    let first = store
        .finish_upload_stage(
            first_stage,
            &session_id,
            "/srv/first.bin",
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let mut second_stage = store.begin_upload_stage().unwrap();
    second_stage.write(b"uvwxyz").unwrap();
    let second = store
        .finish_upload_stage(
            second_stage,
            &session_id,
            "/srv/second.bin",
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE c2_file_transfers SET created_at = ? WHERE id = ?")
        .bind("2026-01-01T00:00:00.000Z")
        .bind(&first.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE c2_file_transfers SET created_at = ? WHERE id = ?")
        .bind("2026-01-01T00:00:01.000Z")
        .bind(&second.id)
        .execute(&repo.pool)
        .await
        .unwrap();

    let initial = store.next_upload_chunks(&session_id, 4).await.unwrap();
    assert_eq!(
        initial.iter().map(|chunk| chunk.data.len()).sum::<usize>(),
        4
    );
    assert!(
        initial
            .iter()
            .all(|chunk| chunk.transfer_id == Some(uuid::Uuid::parse_str(&first.id).unwrap()))
    );
    let ack = nw_profile::msgs::FileAck {
        transfer_id: Some(uuid::Uuid::parse_str(&first.id).unwrap()),
        received: 4,
        total: 6,
        done: false,
    };
    store.ack_upload(&session_id, &ack).await.unwrap();

    let reopened = TransferStore::new(repo, directory.path(), 4096).unwrap();
    let resumed = reopened.next_upload_chunks(&session_id, 4).await.unwrap();
    assert_eq!(resumed[0].offset, 4);
    assert_eq!(resumed[0].data, b"ef");
    reopened
        .ack_upload(
            &session_id,
            &nw_profile::msgs::FileAck {
                transfer_id: ack.transfer_id,
                received: 6,
                total: 6,
                done: true,
            },
        )
        .await
        .unwrap();
    reopened
        .ack_upload(
            &session_id,
            &nw_profile::msgs::FileAck {
                transfer_id: ack.transfer_id,
                received: 6,
                total: 6,
                done: true,
            },
        )
        .await
        .unwrap();
    let next = reopened.next_upload_chunks(&session_id, 2).await.unwrap();
    assert!(
        next.iter()
            .all(|chunk| chunk.transfer_id == Some(uuid::Uuid::parse_str(&second.id).unwrap()))
    );
}

#[tokio::test]
async fn transfer_tasks_are_delivered_one_head_per_direction_until_terminal() {
    let (_directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let first_download = store
        .queue_download(
            &session_id,
            "/srv/first.bin",
            Some(1),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let second_download = store
        .queue_download(
            &session_id,
            "/srv/second.bin",
            Some(1),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let mut stage = store.begin_upload_stage().unwrap();
    stage.write(b"a").unwrap();
    let first_upload = store
        .finish_upload_stage(
            stage,
            &session_id,
            "/srv/first-upload.bin",
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let mut stage = store.begin_upload_stage().unwrap();
    stage.write(b"b").unwrap();
    let second_upload = store
        .finish_upload_stage(
            stage,
            &session_id,
            "/srv/second-upload.bin",
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();

    // Make the FIFO contract explicit instead of depending on UUID ordering
    // when several rows share SQLite's millisecond timestamp.
    for (transfer, minute) in [
        (&first_download, "01"),
        (&second_download, "02"),
        (&first_upload, "03"),
        (&second_upload, "04"),
    ] {
        sqlx::query("UPDATE c2_file_transfers SET created_at = ? WHERE id = ?")
            .bind(format!("2026-09-11T00:{minute}:00.000Z"))
            .bind(&transfer.id)
            .execute(&repo.pool)
            .await
            .unwrap();
    }

    let delivered = repo.tasks_for_delivery(&session_id).await.unwrap();
    assert_eq!(
        delivered
            .iter()
            .filter(|task| task.id == first_download.task_id.clone().unwrap())
            .count(),
        1
    );
    assert_eq!(
        delivered
            .iter()
            .filter(|task| task.id == first_upload.task_id.clone().unwrap())
            .count(),
        1
    );
    assert!(
        !delivered
            .iter()
            .any(|task| task.id == second_download.task_id.clone().unwrap())
    );
    assert!(
        !delivered
            .iter()
            .any(|task| task.id == second_upload.task_id.clone().unwrap())
    );

    let first_ids = delivered
        .iter()
        .map(|task| uuid::Uuid::parse_str(&task.id).unwrap())
        .collect::<Vec<_>>();
    repo.acknowledge_tasks(&session_id, &first_ids)
        .await
        .unwrap();
    assert!(
        repo.tasks_for_delivery(&session_id)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE c2_file_transfers SET status = 'completed' WHERE id IN (?, ?)")
        .bind(&first_download.id)
        .bind(&first_upload.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    let next = repo.tasks_for_delivery(&session_id).await.unwrap();
    assert!(
        next.iter()
            .any(|task| task.id == second_download.task_id.clone().unwrap())
    );
    assert!(
        next.iter()
            .any(|task| task.id == second_upload.task_id.clone().unwrap())
    );
}

#[tokio::test]
async fn cancelling_a_pending_transfer_task_cancels_its_transfer_atomically() {
    let (_directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/cancel.bin",
            Some(1),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    assert_eq!(
        repo.request_task_cancellation(
            &session_id,
            transfer.task_id.as_deref().unwrap(),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap(),
        naughtywolf::db::models::TaskCancellation::Cancelled
    );
    assert_eq!(
        repo.file_transfer(&transfer.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
}

#[tokio::test]
async fn cancelling_a_processing_transfer_releases_fifo_for_the_next_transfer() {
    let (_directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let first = store
        .queue_download(
            &session_id,
            "/srv/processing-cancel.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let second = store
        .queue_download(
            &session_id,
            "/srv/after-cancel.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE c2_file_transfers SET created_at = ? WHERE id = ?")
        .bind("2026-01-01T00:00:00.000Z")
        .bind(&first.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE c2_file_transfers SET created_at = ? WHERE id = ?")
        .bind("2026-01-01T00:00:01.000Z")
        .bind(&second.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    let delivered = repo.tasks_for_delivery(&session_id).await.unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].id, first.task_id.clone().unwrap());
    let first_id = uuid::Uuid::parse_str(&delivered[0].id).unwrap();
    repo.acknowledge_tasks(&session_id, &[first_id])
        .await
        .unwrap();
    let cancellation = repo
        .request_task_cancellation(
            &session_id,
            &first_id.to_string(),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    assert!(matches!(
        cancellation,
        naughtywolf::db::models::TaskCancellation::Requested { .. }
    ));
    // This is the terminal result emitted by the real transfer task after
    // nw/killtask aborts its I/O slot; the repository must atomically cancel
    // the transfer and make the next FIFO row deliverable.
    repo.store_task_result_for_session(
        &session_id,
        &first_id.to_string(),
        false,
        b"",
        b"task cancelled",
        -1,
    )
    .await
    .unwrap();
    assert_eq!(
        repo.file_transfer(&first.id).await.unwrap().unwrap().status,
        "cancelled"
    );
    let next = repo.tasks_for_delivery(&session_id).await.unwrap();
    assert!(
        next.iter()
            .any(|task| task.id == second.task_id.clone().unwrap())
    );
    assert!(
        !next
            .iter()
            .any(|task| task.id == first.task_id.clone().unwrap())
    );
}

#[tokio::test]
async fn final_download_chunk_stays_provisional_until_task_result() {
    let (directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let expected = hex::encode(Sha256::digest(b"abcdef"));
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/provisional.bin",
            Some(6),
            Some(&expected),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let ack = store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 6, b"abcdef"))
        .await
        .unwrap();
    assert!(ack.done);
    assert_eq!(
        repo.file_transfer(&transfer.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "active",
        "receiver completion must remain provisional until task confirmation"
    );
    repo.store_task_result_for_session(
        &session_id,
        transfer.task_id.as_deref().unwrap(),
        true,
        b"download confirmed",
        b"",
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        repo.file_transfer(&transfer.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
    assert!(
        directory
            .path()
            .join("callback-transfers")
            .join(&transfer.storage_key)
            .is_file()
    );
}

#[tokio::test]
async fn concurrent_store_instances_serialize_same_transfer() {
    let (directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/concurrent.bin",
            Some(6),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let second = TransferStore::new(repo, directory.path(), 4096).unwrap();
    let first_chunk = transfer_chunk(&transfer, 0, 6, b"abc");
    let (left, right) = tokio::join!(
        store.receive_chunk(&session_id, &first_chunk),
        second.receive_chunk(&session_id, &first_chunk),
    );
    assert!(left.is_ok() && right.is_ok());
    let received = left.unwrap().received.max(right.unwrap().received);
    assert_eq!(received, 3);
}

#[cfg(unix)]
#[tokio::test]
async fn cross_store_lock_precedes_part_write() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    let (directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/locked.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let second = TransferStore::new(repo.clone(), directory.path(), 4096).unwrap();
    let lock_path = directory
        .path()
        .join("callback-transfers")
        .join(format!("{}.lock", transfer.storage_key));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(lock_path)
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );

    let blocked = second
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 3, b"new"))
        .await;

    assert!(matches!(blocked, Err(TransferError::Storage)));
    assert!(
        !directory
            .path()
            .join("callback-transfers")
            .join(format!("{}.part", transfer.storage_key))
            .exists(),
        "a blocked writer must not create or alter the staging artifact"
    );
    assert_eq!(
        repo.file_transfer(&transfer.id)
            .await
            .unwrap()
            .unwrap()
            .received_bytes,
        0,
        "a blocked writer must not advance the durable offset"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cleanup_does_not_delete_locked_upload_stage() {
    let (directory, repo, store, _operator, _session_id) = transfer_fixture(4096).await;
    let mut stage = store.begin_upload_stage().unwrap();
    stage.write(b"pending").unwrap();
    let second = TransferStore::new(repo, directory.path(), 4096).unwrap();

    assert!(matches!(
        second.cleanup_orphans().await,
        Err(TransferError::Storage)
    ));
    assert!(
        std::fs::read_dir(directory.path().join("callback-transfers"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| std::fs::read(entry.path()).ok().as_deref() == Some(b"pending"))
    );
}

#[cfg(windows)]
#[tokio::test]
async fn cross_store_lock_precedes_part_write() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let (directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/locked.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let second = TransferStore::new(repo.clone(), directory.path(), 4096).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(
            directory
                .path()
                .join("callback-transfers")
                .join(format!("{}.lock", transfer.storage_key)),
        )
        .unwrap();
    let mut overlapped = OVERLAPPED::default();
    assert_ne!(
        unsafe {
            LockFileEx(
                lock.as_raw_handle() as _,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        },
        0
    );

    assert!(matches!(
        second
            .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 3, b"new"))
            .await,
        Err(TransferError::Storage)
    ));
    assert!(
        !directory
            .path()
            .join("callback-transfers")
            .join(format!("{}.part", transfer.storage_key))
            .exists()
    );
    assert_eq!(
        repo.file_transfer(&transfer.id)
            .await
            .unwrap()
            .unwrap()
            .received_bytes,
        0
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn unix_cleanup_closes_directory_on_invalid_name() {
    use std::os::unix::ffi::OsStringExt;

    let (directory, _repo, store, _operator, _session_id) = transfer_fixture(4096).await;
    let invalid = std::ffi::OsString::from_vec(vec![0xff]);
    std::fs::write(
        directory.path().join("callback-transfers").join(invalid),
        b"orphan",
    )
    .unwrap();

    let descriptors_before = std::fs::read_dir("/proc/self/fd").unwrap().count();
    for _ in 0..8 {
        assert!(matches!(
            store.cleanup_orphans().await,
            Err(TransferError::UnsafeStorage)
        ));
    }
    let descriptors_after = std::fs::read_dir("/proc/self/fd").unwrap().count();
    assert_eq!(descriptors_after, descriptors_before);
}

#[tokio::test]
async fn active_file_lease_blocks_takeover_until_safe_expiry() {
    let (directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/lease.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO c2_transfer_leases (transfer_id, owner, expires_at) VALUES (?, ?, unixepoch() + 30)",
    )
    .bind(&transfer.id)
    .bind("other-store")
    .execute(&repo.pool)
    .await
    .unwrap();
    let blocked = store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 3, b"a"))
        .await;
    assert!(matches!(blocked, Err(TransferError::Repository)));
    sqlx::query("UPDATE c2_transfer_leases SET expires_at = unixepoch() - 1 WHERE transfer_id = ?")
        .bind(&transfer.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    let resumed = store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 3, b"a"))
        .await
        .unwrap();
    assert_eq!(resumed.received, 1);
    drop(directory);
}

#[tokio::test]
async fn cleanup_expires_terminal_artifacts_by_configured_timestamp() {
    let directory = tempfile::tempdir().unwrap();
    let repo = test_repository().await;
    let operator = create_user(
        &repo,
        &format!("retention-{}", uuid::Uuid::new_v4()),
        Role::Operator,
    )
    .await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let store = TransferStore::new_with_retention(repo.clone(), directory.path(), 4096, 0).unwrap();
    let mut stage = store.begin_upload_stage().unwrap();
    stage.write(b"expired").unwrap();
    let transfer = store
        .finish_upload_stage(
            stage,
            &session_id,
            "/srv/expired.bin",
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE c2_file_transfers SET status = 'completed', completed_at = '2020-01-01T00:00:00.000Z' WHERE id = ?",
    )
    .bind(&transfer.id)
    .execute(&repo.pool)
    .await
    .unwrap();
    store.cleanup_orphans().await.unwrap();
    assert!(repo.file_transfer(&transfer.id).await.unwrap().is_none());
    assert!(
        !directory
            .path()
            .join("callback-transfers")
            .join(&transfer.storage_key)
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn storage_writes_follow_held_directory_after_ancestor_swap() {
    let evidence = tempfile::tempdir().unwrap();
    let repo = test_repository().await;
    let operator = create_user(
        &repo,
        &format!("ancestor-{}", uuid::Uuid::new_v4()),
        Role::Operator,
    )
    .await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let store = TransferStore::new(repo.clone(), evidence.path(), 4096).unwrap();
    let original = evidence.path().join("callback-transfers");
    let moved = evidence.path().join("callback-transfers-old");
    std::fs::rename(&original, &moved).unwrap();
    std::fs::create_dir(&original).unwrap();
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/ancestor.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 3, b"a"))
        .await
        .unwrap();
    assert!(
        moved
            .join(format!("{}.part", transfer.storage_key))
            .is_file()
    );
    assert!(
        !original
            .join(format!("{}.part", transfer.storage_key))
            .exists()
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_guards_reparse_ancestors_until_publication() {
    let evidence = tempfile::tempdir().unwrap();
    let repo = test_repository().await;
    let operator = create_user(
        &repo,
        &format!("windows-ancestor-{}", uuid::Uuid::new_v4()),
        Role::Operator,
    )
    .await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let store = TransferStore::new(repo.clone(), evidence.path(), 4096).unwrap();
    let original = evidence.path().join("callback-transfers");
    let moved = evidence.path().join("callback-transfers-old");

    assert!(
        std::fs::rename(&original, &moved).is_err(),
        "the retained root/ancestor handles must deny an attacker the delete share needed to replace the publication parent with a reparse point"
    );
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/ancestor.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    store
        .receive_chunk(&session_id, &transfer_chunk(&transfer, 0, 3, b"a"))
        .await
        .unwrap();
    assert!(original.join(&transfer.storage_key).is_file());
}

#[tokio::test]
async fn transfer_endpoints_require_role_scope_and_csrf_and_audit_exact_destination() {
    let (_directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let app = authenticated_transfer_app(repo.clone(), operator, store.clone()).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/download"))
                .header("content-type", "application/json")
                .header("x-csrf-token", csrf)
                .body(Body::from(
                    r#"{"path":"/srv/exact artifact.bin","expected_size":3072}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let transfer = json(response).await;
    assert_eq!(transfer["remote_path"], "/srv/exact artifact.bin");
    let audit: String =
        sqlx::query_scalar("SELECT details FROM c2_audit WHERE action = 'file_download_enqueued'")
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert!(audit.contains("exact destination /srv/exact artifact.bin"));

    let rejected = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/download"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"path":"/srv/no-csrf.bin"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

    let viewer = create_user(&repo, "transfer-viewer", Role::Viewer).await;
    let viewer_app = authenticated_transfer_app(repo, viewer, store).await;
    let rejected = viewer_app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/download"))
                .header("content-type", "application/json")
                .header("x-csrf-token", "irrelevant")
                .body(Body::from(r#"{"path":"/srv/forbidden.bin"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn multipart_upload_streams_bounded_fixture_and_completed_download_has_safe_headers() {
    let (_directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let app = authenticated_transfer_app(repo.clone(), operator.clone(), store.clone()).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let boundary = "nw-transfer-boundary";
    let fixture = vec![0x5a; 3072];
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"destination\"\r\n\r\n/srv/upload target.bin\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"../../ignored.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    body.extend_from_slice(&fixture);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/upload"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header("x-csrf-token", &csrf)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let uploaded = json(response).await;
    assert_eq!(uploaded["expected_size"], 3072);
    assert_eq!(uploaded["remote_path"], "/srv/upload target.bin");
    assert_eq!(uploaded["sha256"], hex::encode(Sha256::digest(&fixture)));

    let expected = hex::encode(Sha256::digest(&fixture));
    let download = store
        .queue_download(
            &session_id,
            "/srv/unsafe\"\r\nname.bin",
            Some(3072),
            Some(&expected),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    store
        .receive_chunk(&session_id, &transfer_chunk(&download, 0, 3072, &fixture))
        .await
        .unwrap();
    repo.store_task_result_for_session(
        &session_id,
        download.task_id.as_deref().unwrap(),
        true,
        b"download confirmed",
        b"",
        0,
    )
    .await
    .unwrap();
    let response = app
        .oneshot(
            Request::get(format!(
                "/api/callbacks/{session_id}/transfers/{}/download",
                download.id
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"download-")
    );
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
        fixture.as_slice()
    );
}

#[tokio::test]
async fn file_backed_application_transfer_survives_http_disconnect_and_db_reopen() {
    let workspace = tempfile::tempdir().unwrap();
    let db_path = workspace.path().join("transfers.sqlite");
    std::fs::File::create(&db_path).unwrap();
    let database_url = format!("sqlite://{}", db_path.display());
    let pool = db::create_pool(&database_url).await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repo = Repository { pool };
    let operator = create_user(&repo, "file-backed-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let storage = workspace.path().join("evidence");
    let store = TransferStore::new(repo.clone(), &storage, 4096).unwrap();
    let upload_bytes = vec![0x31; 3072];
    let download_bytes: Vec<u8> = (0..3072).map(|index| (index % 251) as u8).collect();
    let download_sha = hex::encode(Sha256::digest(&download_bytes));

    // Use an actual TCP listener for the operator API. Aborting this server
    // after the upload models a dropped HTTP connection before the callback
    // resumes from its durable transfer state.
    let app = authenticated_transfer_app(repo.clone(), operator.clone(), store.clone()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(axum::serve(listener, app.into_make_service()).into_future());
    let client = reqwest::Client::new();
    let page = client
        .get(format!("http://{address}/callbacks/{session_id}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = csrf_token(&page);
    let form = reqwest::multipart::Form::new()
        .text("destination", "/srv/file-backed-upload.bin")
        .part(
            "file",
            reqwest::multipart::Part::bytes(upload_bytes.clone())
                .file_name("payload.bin")
                .mime_str("application/octet-stream")
                .unwrap(),
        );
    let upload_response = client
        .post(format!(
            "http://{address}/api/callbacks/{session_id}/files/upload"
        ))
        .header("x-csrf-token", csrf)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(upload_response.status(), reqwest::StatusCode::OK);
    let upload_view: Value = upload_response.json().await.unwrap();
    let upload_id = upload_view["id"].as_str().unwrap().to_owned();
    let upload_task_id = upload_view["task_id"].as_str().unwrap().to_owned();
    let upload = repo.file_transfer(&upload_id).await.unwrap().unwrap();
    assert_eq!(upload.expected_size, Some(3072));
    assert_eq!(
        upload.sha256.as_deref(),
        Some(hex::encode(Sha256::digest(&upload_bytes)).as_str())
    );
    let first_upload_chunk = store
        .next_upload_chunks(&session_id, 1024)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    store
        .ack_upload(
            &session_id,
            &FileAck {
                transfer_id: first_upload_chunk.transfer_id,
                received: first_upload_chunk.data.len() as u64,
                total: first_upload_chunk.total,
                done: false,
            },
        )
        .await
        .unwrap();
    server.abort();
    let _ = server.await;
    repo.pool.close().await;

    // Reconstruct both Repository and TransferStore from the same SQLite file
    // and durable artifact directory; the remaining upload bytes resume at
    // offset 1024 rather than restarting.
    let pool = db::create_pool(&database_url).await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repo = Repository { pool };
    let store = TransferStore::new(repo.clone(), &storage, 4096).unwrap();
    let resumed_upload = store.next_upload_chunks(&session_id, 4096).await.unwrap();
    assert_eq!(resumed_upload.first().unwrap().offset, 1024);
    for chunk in &resumed_upload {
        store
            .ack_upload(
                &session_id,
                &FileAck {
                    transfer_id: chunk.transfer_id,
                    received: chunk.offset + chunk.data.len() as u64,
                    total: chunk.total,
                    done: chunk.offset + chunk.data.len() as u64 == chunk.total,
                },
            )
            .await
            .unwrap();
    }
    repo.store_task_result_for_session(&session_id, &upload_task_id, true, b"published", b"", 0)
        .await
        .unwrap();
    assert_eq!(
        repo.file_transfer(&upload_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );

    let download = store
        .queue_download(
            &session_id,
            "/srv/file-backed-download.bin",
            Some(download_bytes.len() as u64),
            Some(&download_sha),
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    store
        .receive_chunk(
            &session_id,
            &transfer_chunk(&download, 0, 3072, &download_bytes[..1024]),
        )
        .await
        .unwrap();
    repo.pool.close().await;

    let pool = db::create_pool(&database_url).await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repo = Repository { pool };
    let store = TransferStore::new(repo.clone(), &storage, 4096).unwrap();
    let resumed = repo.file_transfer(&download.id).await.unwrap().unwrap();
    assert_eq!(resumed.received_bytes, 1024);
    store
        .receive_chunk(
            &session_id,
            &transfer_chunk(&download, 1024, 3072, &download_bytes[1024..2048]),
        )
        .await
        .unwrap();
    let final_ack = store
        .receive_chunk(
            &session_id,
            &transfer_chunk(&download, 2048, 3072, &download_bytes[2048..]),
        )
        .await
        .unwrap();
    assert!(final_ack.done);
    assert_eq!(
        repo.file_transfer(&download.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "active"
    );
    repo.store_task_result_for_session(
        &session_id,
        download.task_id.as_deref().unwrap(),
        true,
        b"published",
        b"",
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        repo.file_transfer(&download.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
    let queued = repo.tasks_for_delivery(&session_id).await.unwrap();
    assert!(
        !queued
            .iter()
            .any(|task| task.id == download.task_id.clone().unwrap())
    );
    let follower = store
        .queue_download(
            &session_id,
            "/srv/file-backed-follower.bin",
            Some(1),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let ready = repo.tasks_for_delivery(&session_id).await.unwrap();
    assert!(
        ready
            .iter()
            .any(|task| task.id == follower.task_id.clone().unwrap())
    );

    // Recreate the application after the second disconnect and verify the
    // published bytes through the authenticated HTTP download endpoint.
    let app = authenticated_transfer_app(repo.clone(), operator, store).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(axum::serve(listener, app.into_make_service()).into_future());
    let response = client
        .get(format!(
            "http://{address}/api/callbacks/{session_id}/transfers/{}/download",
            download.id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        download_bytes.as_slice()
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn transfer_sse_emits_durable_progress_snapshots() {
    let (_directory, repo, store, operator, session_id) = transfer_fixture(4096).await;
    let transfer = store
        .queue_download(
            &session_id,
            "/srv/progress.bin",
            Some(3),
            None,
            &operator.id,
            &operator.username,
        )
        .await
        .unwrap();
    let app = authenticated_transfer_app(repo, operator, store).await;
    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut output = String::new();
    for _ in 0..4 {
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        output.push_str(std::str::from_utf8(&bytes).unwrap());
        if output.contains("event: transfer") {
            break;
        }
    }
    assert!(output.contains("event: transfer"));
    assert!(output.contains(&transfer.id));
    assert!(output.contains("/srv/progress.bin"));
}

fn process_snapshot_fixture(captured_at: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": "nw.process-list.v1",
        "captured_at": captured_at,
        "processes": [{
            "pid": 7331,
            "parent_pid": 1,
            "name": "safe-worker",
            "executable": "/opt/lab/safe-worker",
            "user": "operator",
            "architecture": "x86_64",
            "cpu_percent": 2.5,
            "memory_bytes": 8192,
            "started_at": "2026-09-10T11:00:00Z"
        }]
    }))
    .unwrap()
}

fn filesystem_snapshot_fixture(path: &str, captured_at: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": "nw.fs-list.v1",
        "captured_at": captured_at,
        "path": path,
        "entries": [{
            "name": "<img src=x onerror=alert(1)>.txt",
            "path": format!("{path}/<img src=x onerror=alert(1)>.txt"),
            "kind": "file",
            "size": 4,
            "modified_at": "2026-09-10T11:00:00Z",
            "permissions": "0644",
            "owner": "operator"
        }]
    }))
    .unwrap()
}

fn padded_filesystem_result(schema: &str, total_bytes: usize) -> Vec<u8> {
    let prefix = format!(
        r#"{{"schema":"{schema}","captured_at":"2026-09-10T12:36:00Z","path":"/srv/files","entries":[],"padding":""#
    );
    let suffix = r#""}"#;
    assert!(prefix.len() + suffix.len() <= total_bytes);
    format!(
        "{prefix}{}{suffix}",
        "x".repeat(total_bytes - prefix.len() - suffix.len())
    )
    .into_bytes()
}

async fn login(session: Session, Extension(user): Extension<AuthenticatedUser>) -> StatusCode {
    AuthSession { session }.login(&user).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn authenticated_app(repository: Repository, user: AuthenticatedUser) -> Router {
    let app = Router::<Repository>::new()
        .route("/test/login", post(login))
        .merge(portal::authenticated_router())
        .with_state(repository)
        .layer(Extension(user))
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false));
    let response = app
        .clone()
        .oneshot(Request::post("/test/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Router::new().fallback_service(
        tower::ServiceBuilder::new()
            .map_request(move |mut request: Request<Body>| {
                request
                    .headers_mut()
                    .insert("cookie", cookie.parse().unwrap());
                request
            })
            .service(app),
    )
}

async fn authenticated_transfer_app(
    repository: Repository,
    user: AuthenticatedUser,
    store: TransferStore,
) -> Router {
    let app = Router::<Repository>::new()
        .route("/test/login", post(login))
        .merge(portal::authenticated_router())
        .with_state(repository)
        .layer(Extension(store))
        .layer(Extension(user))
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false));
    let response = app
        .clone()
        .oneshot(Request::post("/test/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Router::new().fallback_service(
        tower::ServiceBuilder::new()
            .map_request(move |mut request: Request<Body>| {
                request
                    .headers_mut()
                    .insert("cookie", cookie.parse().unwrap());
                request
            })
            .service(app),
    )
}

async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn response_text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn csrf_token(page: &str) -> String {
    page.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("callback page csrf token")
        .to_owned()
}

#[tokio::test]
async fn task_api_paginates_on_created_at_and_id_without_overlap() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "page-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;

    for index in 0..35 {
        let id = repo
            .enqueue_task(
                &session_id,
                &format!("command-{index:02}"),
                &serde_json::json!([index]),
                30_000,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE c2_tasks SET created_at = ?, updated_at = ? WHERE id = ?")
            .bind(format!("2026-09-10T12:{index:02}:00.000Z"))
            .bind(format!("2026-09-10T12:{index:02}:00.000Z"))
            .bind(id)
            .execute(&repo.pool)
            .await
            .unwrap();
    }

    let app = authenticated_app(repo, operator).await;
    let first = app
        .clone()
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/tasks?limit=20"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = json(first).await;
    assert_eq!(first["tasks"].as_array().unwrap().len(), 20);
    assert_eq!(first["tasks"][0]["command"], "command-34");
    let before = first["next_before"].as_str().expect("first page cursor");
    assert!(!before.contains("2026-09-10"), "cursor must be opaque");

    let second = app
        .oneshot(
            Request::get(format!(
                "/api/callbacks/{session_id}/tasks?limit=20&before={before}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second = json(second).await;
    assert_eq!(second["tasks"].as_array().unwrap().len(), 15);
    assert!(second["next_before"].is_null());
    let first_ids = first["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert!(
        second["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| !first_ids.contains(task["id"].as_str().unwrap()))
    );
}

#[tokio::test]
async fn process_list_result_validates_and_persists_projection_atomically() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-projection", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    let stdout = process_snapshot_fixture("2026-09-10T12:34:56Z");

    assert!(
        repo.store_task_result_for_session(&session_id, &task_id, true, &stdout, &[], 0)
            .await
            .unwrap()
    );
    let snapshot = repo
        .latest_process_snapshot(&session_id)
        .await
        .unwrap()
        .expect("persisted process projection");
    assert_eq!(snapshot.task_id, task_id);
    assert_eq!(snapshot.schema_version, "nw.process-list.v1");
    assert_eq!(snapshot.captured_at, "2026-09-10T12:34:56Z");
    assert_eq!(snapshot.snapshot_json["processes"][0]["pid"], 7331);
}

#[tokio::test]
async fn process_list_result_rejects_unknown_schema_without_storing_any_result() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-invalid", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    let invalid =
        br#"{"schema":"nw.process-list.v2","captured_at":"2026-09-10T12:34:56Z","processes":[]}"#;

    let error = repo
        .store_task_result_for_session(&session_id, &task_id, true, invalid, &[], 0)
        .await
        .unwrap_err();
    assert!(matches!(error, naughtywolf::AppError::Validation(_)));
    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    let snapshot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_process_snapshots WHERE session_id = ?")
            .bind(&session_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!((result_count, snapshot_count), (0, 0));
}

#[tokio::test]
async fn older_process_snapshot_result_cannot_replace_newer_projection() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-order", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let newer_task = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    let older_task = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    repo.store_task_result_for_session(
        &session_id,
        &newer_task,
        true,
        &process_snapshot_fixture("2026-09-10T12:35:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();
    repo.store_task_result_for_session(
        &session_id,
        &older_task,
        true,
        &process_snapshot_fixture("2026-09-10T12:34:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();

    let snapshot = repo
        .latest_process_snapshot(&session_id)
        .await
        .unwrap()
        .expect("newer process projection");
    assert_eq!(snapshot.task_id, newer_task);
    assert_eq!(snapshot.captured_at, "2026-09-10T12:35:00Z");
}

#[tokio::test]
async fn filesystem_list_result_atomically_upserts_normalized_monotonic_projection() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-projection", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let newer_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv//lab/../files/"]),
            30_000,
        )
        .await
        .unwrap();
    let older_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();

    repo.store_task_result_for_session(
        &session_id,
        &newer_task,
        true,
        &filesystem_snapshot_fixture("/srv/files", "2026-09-10T12:35:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();
    repo.store_task_result_for_session(
        &session_id,
        &older_task,
        true,
        &filesystem_snapshot_fixture("/srv/files", "2026-09-10T12:34:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();

    let snapshot = repo
        .latest_file_snapshot(&session_id, "/srv/files")
        .await
        .unwrap()
        .expect("persisted filesystem projection");
    assert_eq!(snapshot.path, "/srv/files");
    assert_eq!(snapshot.task_id, newer_task);
    assert_eq!(snapshot.schema_version, "nw.fs-list.v1");
    assert_eq!(snapshot.captured_at, "2026-09-10T12:35:00Z");
    assert_eq!(snapshot.snapshot_json["entries"][0]["size"], 4);
}

#[tokio::test]
async fn filesystem_list_result_rejects_path_mismatch_without_any_result_write() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-invalid", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();

    let error = repo
        .store_task_result_for_session(
            &session_id,
            &task_id,
            true,
            &filesystem_snapshot_fixture("/srv/other", "2026-09-10T12:35:00Z"),
            &[],
            0,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, naughtywolf::AppError::Validation(_)));
    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    let snapshot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_file_snapshots WHERE session_id = ?")
            .bind(&session_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!((result_count, snapshot_count), (0, 0));
}

#[tokio::test]
async fn filesystem_list_result_rejects_nonchild_and_name_mismatch_entries_atomically() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-entry-invalid", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;

    for entry in [
        serde_json::json!({"name":"report.txt","path":"/srv/files/nested/report.txt"}),
        serde_json::json!({"name":"other.txt","path":"/srv/files/report.txt"}),
        serde_json::json!({"name":"report.txt","path":"/srv/other/report.txt"}),
    ] {
        let task_id = repo
            .enqueue_task(
                &session_id,
                "nw/fs-list",
                &serde_json::json!(["/srv/files"]),
                30_000,
            )
            .await
            .unwrap();
        let result = serde_json::to_vec(&serde_json::json!({
            "schema":"nw.fs-list.v1",
            "captured_at":"2026-09-10T12:35:00Z",
            "path":"/srv/files",
            "entries":[{
                "name": entry["name"], "path": entry["path"], "kind":"file", "size":4,
                "modified_at":null, "permissions":null, "owner":null
            }]
        }))
        .unwrap();
        let error = repo
            .store_task_result_for_session(&session_id, &task_id, true, &result, &[], 0)
            .await
            .unwrap_err();
        assert!(matches!(error, naughtywolf::AppError::Validation(_)));
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn unknown_filesystem_schema_is_acked_and_stored_raw_without_projection() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-future", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let baseline_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();
    assert!(
        repo.store_task_result_for_session(
            &session_id,
            &baseline_task,
            true,
            &filesystem_snapshot_fixture("/srv/files", "2026-09-10T12:35:00Z"),
            &[],
            0,
        )
        .await
        .unwrap()
    );
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();
    let raw = br#"{"schema":"nw.fs-list.v2","captured_at":"2026-09-10T12:36:00Z","path":"/srv/files","entries":[{"future":true}]}"#;

    assert!(
        repo.store_task_result_for_session(&session_id, &task_id, true, raw, &[], 0)
            .await
            .unwrap(),
        "successful store makes the poll layer ACK this result"
    );
    let stored: (Vec<u8>, String) = sqlx::query_as(
        "SELECT r.stdout, t.status FROM c2_task_results r JOIN c2_tasks t ON t.id = r.task_id WHERE r.task_id = ?",
    )
    .bind(&task_id)
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    assert_eq!(stored.0, raw);
    assert_eq!(stored.1, "completed");
    let terminal_audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM c2_audit WHERE target_session = ? AND action = 'task_completed' AND details = ?",
    )
    .bind(&session_id)
    .bind(format!("task {task_id}"))
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    assert_eq!(terminal_audit, 1);
    let snapshot = repo
        .latest_file_snapshot(&session_id, "/srv/files")
        .await
        .unwrap()
        .expect("existing typed projection remains");
    assert_eq!(snapshot.task_id, baseline_task);
    assert_eq!(snapshot.captured_at, "2026-09-10T12:35:00Z");
}

#[tokio::test]
async fn filesystem_projection_rejects_more_than_the_contract_entry_limit() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-result-limit", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();
    let entries = (0..=nw_profile::control::MAX_FILE_LIST_ENTRIES)
        .map(|index| {
            let name = format!("entry-{index}");
            serde_json::json!({
                "name": name,
                "path": format!("/srv/files/entry-{index}"),
                "kind": "file",
                "size": 0,
                "modified_at": null,
                "permissions": null,
                "owner": null
            })
        })
        .collect::<Vec<_>>();
    let oversized = serde_json::to_vec(&serde_json::json!({
        "schema":"nw.fs-list.v1",
        "captured_at":"2026-09-10T12:35:00Z",
        "path":"/srv/files",
        "entries":entries
    }))
    .unwrap();

    let error = repo
        .store_task_result_for_session(&session_id, &task_id, true, &oversized, &[], 0)
        .await
        .unwrap_err();
    assert!(matches!(error, naughtywolf::AppError::Validation(_)));
    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!(result_count, 0);
}

#[tokio::test]
async fn filesystem_result_byte_cap_is_checked_before_parsing_or_persistence() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-byte-limit", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;

    let boundary_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();
    let boundary =
        padded_filesystem_result("nw.fs-list.v2", nw_profile::control::MAX_FILE_LIST_BYTES);
    assert!(
        repo.store_task_result_for_session(&session_id, &boundary_task, true, &boundary, &[], 0,)
            .await
            .unwrap()
    );
    let stored_boundary: Vec<u8> =
        sqlx::query_scalar("SELECT stdout FROM c2_task_results WHERE task_id = ?")
            .bind(&boundary_task)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!(stored_boundary, boundary);

    let over_limit_payloads = [
        padded_filesystem_result(
            "nw.fs-list.v1",
            nw_profile::control::MAX_FILE_LIST_BYTES + 1,
        ),
        padded_filesystem_result(
            "nw.fs-list.v2",
            nw_profile::control::MAX_FILE_LIST_BYTES + 1,
        ),
        vec![b'!'; nw_profile::control::MAX_FILE_LIST_BYTES + 1],
    ];
    for payload in over_limit_payloads {
        let task_id = repo
            .enqueue_task(
                &session_id,
                "nw/fs-list",
                &serde_json::json!(["/srv/files"]),
                30_000,
            )
            .await
            .unwrap();
        let error = repo
            .store_task_result_for_session(&session_id, &task_id, true, &payload, &[], 0)
            .await
            .unwrap_err();
        match error {
            naughtywolf::AppError::Validation(message) => {
                assert_eq!(message, "filesystem list result exceeds byte limit")
            }
            other => panic!("unexpected error: {other}"),
        }
        let result_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(result_count, 0);
        let task_state: (String, i64) = sqlx::query_as(
            "SELECT status, (SELECT COUNT(*) FROM c2_audit WHERE details = 'task ' || c2_tasks.id AND action = 'task_completed') FROM c2_tasks WHERE id = ?",
        )
        .bind(&task_id)
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(task_state, ("pending".to_owned(), 0));
    }
    assert!(
        repo.latest_file_snapshot(&session_id, "/srv/files")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn filesystem_mutation_result_must_match_the_exact_task_target_atomically() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-result", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/fs-delete",
            &serde_json::json!(["/srv/exact", "true"]),
            30_000,
        )
        .await
        .unwrap();
    let mismatched = br#"{"schema":"nw.fs-mutation.v1","action":"delete","path":"/srv/other","destination":null,"recursive":true,"completed_at":"2026-09-10T12:35:00Z"}"#;

    let error = repo
        .store_task_result_for_session(&session_id, &task_id, true, mismatched, &[], 0)
        .await
        .unwrap_err();
    assert!(matches!(error, naughtywolf::AppError::Validation(_)));
    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!(result_count, 0);

    let valid_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-delete",
            &serde_json::json!(["/srv/exact", "true"]),
            30_000,
        )
        .await
        .unwrap();
    let valid = br#"{"schema":"nw.fs-mutation.v1","action":"delete","path":"/srv/exact","destination":null,"recursive":true,"completed_at":"2026-09-10T12:35:01Z"}"#;
    assert!(
        repo.store_task_result_for_session(&session_id, &valid_task, true, valid, &[], 0)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn filesystem_routes_require_role_scope_csrf_and_audit_exact_paths() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-owner", Role::Operator).await;
    let outsider = create_user(&repo, "filesystem-outsider", Role::Operator).await;
    let viewer = create_user(&repo, "filesystem-viewer", Role::Viewer).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let owner_app = authenticated_app(repo.clone(), operator).await;

    let missing_csrf = owner_app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/mkdir"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"path":"/srv/exact <target>"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::BAD_REQUEST);

    let hidden = authenticated_app(repo.clone(), outsider)
        .await
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/delete"))
                .header("content-type", "application/json")
                .header("x-csrf-token", "not-disclosed")
                .body(Body::from(
                    r#"{"path":"/srv/exact <target>","recursive":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let viewer_denied = authenticated_app(repo.clone(), viewer)
        .await
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/list"))
                .header("content-type", "application/json")
                .header("x-csrf-token", "not-disclosed")
                .body(Body::from(r#"{"path":"/srv"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_denied.status(), StatusCode::FORBIDDEN);

    let page = owner_app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let cases = [
        (
            "list",
            r#"{"path":"/srv/files"}"#,
            "nw/fs-list",
            serde_json::json!(["/srv/files"]),
        ),
        (
            "mkdir",
            r#"{"path":"/srv/exact <target>"}"#,
            "nw/fs-mkdir",
            serde_json::json!(["/srv/exact <target>"]),
        ),
        (
            "move",
            r#"{"source":"/srv/exact <target>","destination":"/srv/moved & safe"}"#,
            "nw/fs-move",
            serde_json::json!(["/srv/exact <target>", "/srv/moved & safe"]),
        ),
        (
            "delete",
            r#"{"path":"/srv/moved & safe","recursive":true}"#,
            "nw/fs-delete",
            serde_json::json!(["/srv/moved & safe", "true"]),
        ),
    ];
    for (route, body, command, arguments) in cases {
        let response = owner_app
            .clone()
            .oneshot(
                Request::post(format!("/api/callbacks/{session_id}/files/{route}"))
                    .header("content-type", "application/json")
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let task = json(response).await;
        assert_eq!(task["command"], command);
        assert_eq!(task["arguments"], arguments);
    }
    let audits: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, details FROM c2_audit WHERE target_session = ? AND action LIKE 'filesystem_%' ORDER BY id",
    )
    .bind(&session_id)
    .fetch_all(&repo.pool)
    .await
    .unwrap();
    assert_eq!(audits.len(), 4);
    assert!(
        audits
            .iter()
            .any(|(action, details)| action == "filesystem_mkdir_enqueued"
                && details.contains("exact path /srv/exact <target>"))
    );
    assert!(
        audits
            .iter()
            .any(|(action, details)| action == "filesystem_move_enqueued"
                && details.contains("source /srv/exact <target>")
                && details.contains("destination /srv/moved & safe"))
    );
    assert!(
        audits
            .iter()
            .any(|(action, details)| action == "filesystem_delete_enqueued"
                && details.contains("exact path /srv/moved & safe")
                && details.contains("recursive true"))
    );
}

#[tokio::test]
async fn filesystem_routes_reject_invalid_paths_and_non_boolean_recursion_before_enqueue() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-validation", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);

    for body in [
        r#"{"path":""}"#,
        r#"{"path":"bad\u0000path"}"#,
        r#"{"path":"relative/path"}"#,
        r#"{"path":"/srv/files","recursive":"true"}"#,
        r#"{"path":"/srv/files","recursive":1}"#,
    ] {
        let route = if body.contains("recursive") {
            "delete"
        } else {
            "mkdir"
        };
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/callbacks/{session_id}/files/{route}"))
                    .header("content-type", "application/json")
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "{body}: {}",
            response.status()
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn process_kill_success_enqueues_a_linked_refresh_after_completion() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-refresh", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let kill_id = repo
        .enqueue_task_with_audit(
            &session_id,
            "nw/process-kill",
            &serde_json::json!([7331]),
            30_000,
            &operator.id,
            &operator.username,
            None,
            "process_kill_enqueued",
        )
        .await
        .unwrap();
    let kill_result = br#"{"schema":"nw.process-kill.v1","pid":7331,"name":"safe-worker","terminated":true,"terminated_at":"2026-09-10T12:35:00Z"}"#;

    repo.store_task_result_for_session(&session_id, &kill_id, true, kill_result, &[], 0)
        .await
        .unwrap();

    let refresh: (String, serde_json::Value, String, Option<String>) = sqlx::query_as(
        "SELECT command, args_json, status, parent_task_id FROM c2_tasks WHERE parent_task_id = ?",
    )
    .bind(&kill_id)
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    assert_eq!(refresh.0, "nw/process-list");
    assert_eq!(refresh.1, serde_json::json!([]));
    assert_eq!(refresh.2, "pending");
    assert_eq!(refresh.3.as_deref(), Some(kill_id.as_str()));
}

#[tokio::test]
async fn process_control_mutations_require_scope_and_csrf_and_audit_exact_pid() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-owner", Role::Operator).await;
    let outsider = create_user(&repo, "process-outsider", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let owner_app = authenticated_app(repo.clone(), operator).await;

    let missing_csrf = owner_app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/refresh"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::BAD_REQUEST);

    let missing_kill_csrf = owner_app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/7331/kill"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_kill_csrf.status(), StatusCode::BAD_REQUEST);

    let outsider_app = authenticated_app(repo.clone(), outsider).await;
    let hidden = outsider_app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/7331/kill"))
                .header("x-csrf-token", "not-disclosed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let page = owner_app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let kill = owner_app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/7331/kill"))
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(kill.status(), StatusCode::OK);
    let kill = json(kill).await;
    assert_eq!(kill["command"], "nw/process-kill");
    assert_eq!(kill["arguments"], serde_json::json!(["7331"]));
    let details: String = sqlx::query_scalar(
        "SELECT details FROM c2_audit WHERE target_session = ? AND action = 'process_kill_enqueued'",
    )
    .bind(&session_id)
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    assert!(details.contains("PID 7331"), "audit details: {details}");
}

#[tokio::test]
async fn generic_task_endpoints_reject_reserved_structured_commands_and_retries() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "reserved-process", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);

    for command in [
        "nw/process-list",
        "nw/process-kill",
        "nw/fs-list",
        "nw/fs-stat",
        "nw/fs-mkdir",
        "nw/fs-move",
        "nw/fs-delete",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/callbacks/{session_id}/tasks"))
                    .header("content-type", "application/json")
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(format!(
                        r#"{{"command":"{command}","arguments":[]}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{command}");
    }

    let old_task = repo
        .enqueue_task(
            &session_id,
            "nw/process-kill",
            &serde_json::json!(["7331"]),
            30_000,
        )
        .await
        .unwrap();
    let response = app
        .oneshot(
            Request::post(format!(
                "/api/callbacks/{session_id}/tasks/{old_task}/retry"
            ))
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn task_api_returns_not_found_for_a_callback_outside_operator_scope() {
    let repo = test_repository().await;
    let owner = create_user(&repo, "scope-owner", Role::Operator).await;
    let outsider = create_user(&repo, "scope-outsider", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &owner.id, &session_id).await;
    let app = authenticated_app(repo, outsider).await;

    for path in [
        format!("/api/callbacks/{session_id}/tasks"),
        format!("/callbacks/{session_id}"),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn task_api_propagates_repository_failures_as_server_errors() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "failure-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    sqlx::query("DROP TABLE c2_tasks")
        .execute(&repo.pool)
        .await
        .unwrap();
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/tasks"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn task_api_mutations_require_csrf_and_persist_operator_audit_links() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "mutation-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator.clone()).await;

    let missing_csrf = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/tasks"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"command":"whoami","arguments":[],"timeout_ms":30000}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let enqueue = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/tasks"))
                .header("content-type", "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(
                    r#"{"command":"whoami","arguments":["--all"],"timeout_ms":30000}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enqueue.status(), StatusCode::OK);
    let original = json(enqueue).await;
    assert_eq!(original["operator_id"], operator.id);
    assert_eq!(original["operator_name"], operator.username);

    let retry = app
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/callbacks/{session_id}/tasks/{}/retry",
                original["id"].as_str().unwrap()
            ))
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    let retry = json(retry).await;
    assert_eq!(retry["parent_task_id"], original["id"]);

    let cancel = app
        .oneshot(
            Request::post(format!(
                "/api/callbacks/{session_id}/tasks/{}/cancel",
                retry["id"].as_str().unwrap()
            ))
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::OK);
    assert_eq!(json(cancel).await["status"], "cancelled");

    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM c2_audit WHERE target_session = ? ORDER BY rowid")
            .bind(&session_id)
            .fetch_all(&repo.pool)
            .await
            .unwrap();
    assert_eq!(actions, ["task_enqueued", "task_retried", "task_cancelled"]);
}

#[tokio::test]
async fn enqueue_and_audit_roll_back_together_when_the_audit_write_fails() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "rollback-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    sqlx::query("DROP TABLE c2_audit")
        .execute(&repo.pool)
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/tasks"))
                .header("content-type", "application/json")
                .header("x-csrf-token", csrf)
                .body(Body::from(r#"{"command":"id","arguments":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn task_events_begin_with_an_authoritative_complete_snapshot() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "events-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task_with_audit(
            &session_id,
            "hostname",
            &serde_json::json!(["--fqdn"]),
            30_000,
            &operator.id,
            &operator.username,
            None,
            "task_enqueued",
        )
        .await
        .unwrap();
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .expect("initial SSE snapshot")
        .expect("SSE body item")
        .expect("SSE bytes");
    let event = String::from_utf8(first.to_vec()).unwrap();
    assert!(event.contains("event: task"));
    assert!(event.contains(&task_id));
    assert!(event.contains("\"arguments\":[\"--fqdn\"]"));
    assert!(event.contains("\"status\":\"pending\""));
    assert!(event.contains("\"operator_name\":\"events-operator-user\""));
}

#[tokio::test]
async fn task_events_terminate_when_reconciliation_reads_fail() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "stream-failure-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    repo.enqueue_task(&session_id, "pwd", &serde_json::json!([]), 30_000)
        .await
        .unwrap();
    let app = authenticated_app(repo.clone(), operator).await;
    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .expect("initial task snapshot")
        .expect("initial body item")
        .expect("initial event bytes");
    sqlx::query("DROP TABLE c2_tasks")
        .execute(&repo.pool)
        .await
        .unwrap();

    let next = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("repository failure must not leave a stale stream connected");
    assert!(next.is_none(), "repository failure must terminate SSE");
}

#[tokio::test]
async fn tasking_tab_exposes_persistent_controls_and_local_assets() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "tab-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = response_text(response).await;
    assert!(page.contains("/static/callback-workspace.css"));
    assert!(page.contains("/static/callback-workspace.js"));
    assert!(page.contains("data-callback-tasking"));
    assert!(page.contains(&format!(
        "data-tasks-endpoint=\"/api/callbacks/{session_id}/tasks\""
    )));
    assert!(page.contains(&format!(
        "data-events-endpoint=\"/api/callbacks/{session_id}/events\""
    )));
    assert!(page.contains(&format!(
        "data-process-panel data-processes-endpoint=\"/api/callbacks/{session_id}/processes\""
    )));
    assert!(page.contains("data-process-capable=\"false\""));
    for label in [
        "Tasking",
        "Search task history",
        "State",
        "Errors",
        "Load older",
        "Connection",
    ] {
        assert!(page.contains(label), "missing tasking label {label}");
    }
}

#[tokio::test]
async fn filesystem_workspace_renders_url_navigation_controls_and_disabled_transfers() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-page", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!(
                "/callbacks/{session_id}?tab=files&path=%2Fsrv%2Flab"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = response_text(response).await;
    for marker in [
        "data-file-panel",
        "data-file-path-form",
        "data-file-breadcrumbs",
        "data-file-parent",
        "data-file-refresh",
        "data-file-mkdir-form",
        "data-file-table-body",
        "data-file-confirm",
        "data-file-task-link",
        "data-file-snapshot-age",
    ] {
        assert!(page.contains(marker), "missing {marker}");
    }
    assert!(page.contains("?tab=files&amp;path=%2F"));
    assert_eq!(
        page.matches("Transfer support is being initialized")
            .count(),
        2
    );
    assert!(page.contains("data-file-upload disabled"));
    assert!(page.contains("data-file-download disabled"));
}
