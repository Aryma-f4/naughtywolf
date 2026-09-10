# Mythic-Style Callback Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a persistent Mythic-style callback workspace for Linux and Windows with reliable tasking, metadata, process control, filesystem control, and resumable file transfer.

**Architecture:** SQLite remains authoritative for callbacks, task lifecycle, structured snapshots, audit events, and transfer progress. Versioned typed commands extend the encrypted native C2 protocol; the server-rendered portal loads authoritative pages and reconciles authenticated SSE snapshots by stable IDs. Linux and Windows share Rust interfaces with platform adapters and every mutating action becomes an audited task.

**Tech Stack:** Rust 2024, Axum 0.8, Tokio, SQLx/SQLite, Serde JSON, `sysinfo`, `hostname`, `whoami`, vanilla JavaScript, server-rendered HTML/CSS, GitHub Actions, Docker Compose.

**Spec:** `docs/superpowers/specs/2026-09-10-mythic-callback-workspace-design.md`

## Global Constraints

- Support native NaughtyWolf callbacks on Linux and Windows.
- Keep Rust/SQLite/server-rendered HTML/vanilla JavaScript; do not port Mythic React or GraphQL code.
- SQLite and protected transfer storage are authoritative across navigation, reconnect, and portal restart.
- Use versioned structured JSON for process and filesystem results; never parse localized shell output.
- Pass filesystem paths and process identifiers to typed Rust functions without shell interpolation.
- Require operator-or-admin role, callback visibility, and CSRF validation for every mutation.
- Audit exact callback, operator, task, PID/path, action, outcome, and timestamp without logging secrets.
- Destructive process/filesystem controls require an exact-target confirmation in the UI.
- Older implants may register and task normally; unsupported workspace controls show a capability message.
- Run implementation steps with TDD: observe RED, add minimal behavior, observe GREEN, then commit.

---

## File Structure

- `migrations/004_callback_workspace.sql`: lifecycle, callback metadata, snapshot, and transfer schema.
- `crates/profile/src/control.rs`: cross-platform, versioned process/filesystem/metadata result contracts.
- `crates/profile/src/msgs.rs`: backward-compatible registration fields and transfer identifiers.
- `crates/implant/src/metadata.rs`: OS-backed hostname, username, executable, address, and version discovery.
- `crates/implant/src/processes.rs`: typed process listing and termination.
- `crates/implant/src/filesystem.rs`: typed list/stat/mkdir/move/delete operations.
- `crates/implant/src/runtime.rs`: task acknowledgment, duplicate suppression, typed dispatch, and transfer coordination.
- `src/db/models.rs`: durable task, callback metadata, snapshot, and transfer models.
- `src/db/repositories.rs`: transactional lifecycle, projection, audit, pagination, and transfer queries.
- `src/callback_workspace/mod.rs`: authenticated workspace API router.
- `src/callback_workspace/tasks.rs`: task DTOs, pagination, cancellation, retry, and SSE reconciliation.
- `src/callback_workspace/processes.rs`: process refresh/kill endpoints.
- `src/callback_workspace/files.rs`: filesystem and transfer endpoints.
- `src/callback_workspace/transfers.rs`: protected staging, chunk persistence, checksums, resume, and publication.
- `src/c2.rs`: native poll integration with acknowledgements and transfer chunks.
- `src/portal/templates.rs`: callback workspace shell and progressive server rendering.
- `static/callback-workspace.js`: tab navigation, task reconciliation, filters, confirmations, browsers, and progress.
- `static/callback-workspace.css`: responsive Mythic-inspired workspace styling.
- `tests/callback_workspace_test.rs`: repository, HTTP, RBAC, CSRF, persistence, and rendering integration tests.
- `tests/callback_workspace_test.cjs`: browser-state reconciliation, history, filters, tabs, and dialog behavior tests.
- `crates/implant/tests/callback_controls.rs`: platform-safe process/filesystem/metadata integration tests.
- `.github/workflows/ci.yml`: Linux and Windows verification matrix.
- `README.md`: supervised implant execution and callback workspace operator guide.

### Task 1: Restore Authoritative Task History

**Files:**
- Modify: `src/db/repositories.rs`
- Modify: `src/db/models.rs`
- Modify: `src/portal.rs`
- Test: `tests/repository_test.rs`
- Test: `tests/portal_routes_test.rs`

**Interfaces:**
- Consumes: existing `Repository::list_tasks_for_session(&str)` and `C2TaskWithResult`.
- Produces: `Repository::list_tasks_for_session(&str) -> Result<Vec<C2TaskWithResult>, AppError>` with valid `result_exit_code` and `result_stderr` projections; portal routes propagate repository failures.

- [ ] **Step 1: Add a repository regression test with a stored exit code**

```rust
#[tokio::test]
async fn callback_task_history_projects_the_persisted_exit_code() {
    let (repo, _) = repository_fixture().await;
    let sid = insert_callback_fixture(&repo, "history-host").await;
    let task_id = repo.enqueue_task(&sid, "ls", &serde_json::json!([]), 30_000).await.unwrap();
    repo.store_task_result(&task_id, true, b"one\ntwo\n", b"", 0).await.unwrap();

    let tasks = repo.list_tasks_for_session(&sid).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].result_exit_code, Some(0));
    assert_eq!(tasks[0].result_stderr.as_deref(), Some(&b""[..]));
    assert_eq!(tasks[0].status, "completed");
}
```

- [ ] **Step 2: Run the regression and observe the SQLx column failure**

Run: `cargo test -p naughtywolf --test repository_test callback_task_history_projects_the_persisted_exit_code -- --nocapture`  
Expected: FAIL with `ColumnNotFound("result_exit_code")`.

- [ ] **Step 3: Alias the joined result and stop swallowing database failures**

Change the projection in `list_tasks_for_session` to:

```sql
SELECT t.id, t.session_id, t.command, t.args_json, t.status, t.created_at,
       t.processing_at, t.completed_at, t.result_output, t.result_ok,
       r.exit_code AS result_exit_code,
       r.stderr AS result_stderr
FROM c2_tasks t
LEFT JOIN c2_task_results r ON r.task_id = t.id
WHERE t.session_id = ?
ORDER BY t.created_at DESC, t.id DESC
```

Add `result_stderr: Option<Vec<u8>>` to `C2TaskWithResult`. In both `callback_detail` and `tasks_json`, replace `.await.unwrap_or_default()` with `.await?`.

- [ ] **Step 4: Add a route regression proving persisted rows render after reload**

Create a completed task through the repository, request `/callbacks/{sid}` twice with an authenticated operator session, and assert both responses contain the full task ID, `Completed`, and decoded output.

- [ ] **Step 5: Run focused and portal suites**

Run: `cargo test -p naughtywolf --test repository_test callback_task_history_projects_the_persisted_exit_code && cargo test -p naughtywolf --test portal_routes_test`  
Expected: PASS with zero failures.

- [ ] **Step 6: Commit the persistence fix**

```bash
git add src/db/models.rs src/db/repositories.rs src/portal.rs tests/repository_test.rs tests/portal_routes_test.rs
git commit -m "fix: restore persisted callback task history"
```

### Task 2: Add Durable Workspace Schema and Models

**Files:**
- Create: `migrations/004_callback_workspace.sql`
- Modify: `src/db/models.rs`
- Modify: `src/db/repositories.rs`
- Test: `tests/config_db_test.rs`

**Interfaces:**
- Consumes: current `callbacks`, `c2_tasks`, `c2_task_results`, and `c2_audit` tables.
- Produces: `ProcessSnapshot`, `FileSnapshot`, `FileTransfer`, and lifecycle columns used by all later tasks.

- [ ] **Step 1: Write a migration contract test**

```rust
#[tokio::test]
async fn callback_workspace_schema_persists_lifecycle_snapshots_and_transfers() {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let task_columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('c2_tasks') ORDER BY cid"
    ).fetch_all(&pool).await.unwrap();
    assert!(task_columns.contains(&"operator_id".into()));
    assert!(task_columns.contains(&"parent_task_id".into()));
    assert!(task_columns.contains(&"updated_at".into()));
    assert!(task_columns.contains(&"cancellation_requested_at".into()));
    for table in ["c2_process_snapshots", "c2_file_snapshots", "c2_file_transfers"] {
        let found: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?"
        ).bind(table).fetch_one(&pool).await.unwrap();
        assert_eq!(found, 1, "missing {table}");
    }
}
```

- [ ] **Step 2: Run the migration test and observe missing columns/tables**

Run: `cargo test -p naughtywolf --test config_db_test callback_workspace_schema_persists_lifecycle_snapshots_and_transfers -- --nocapture`  
Expected: FAIL because `operator_id` is absent.

- [ ] **Step 3: Create migration 004**

Add nullable backward-compatible callback columns `os_version`, `executable_path`, `local_addr`, `implant_version`, `interval_ms`, `jitter_ms`, and `capabilities_json`. Rebuild `c2_tasks` through `c2_tasks_v2` so its state constraint accepts `cancelled`, adding `operator_id`, `parent_task_id`, `updated_at`, and `cancellation_requested_at`. Before replacing the parent table, copy `c2_task_results` into `c2_task_results_backup`; recreate its foreign key against the renamed v2 table and restore every result row. The migration test must compare task/result counts and bytes before and after migration. Create:

```sql
CREATE TABLE c2_process_snapshots (
    session_id TEXT PRIMARY KEY REFERENCES c2_sessions(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES c2_tasks(id) ON DELETE CASCADE,
    schema_version TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    captured_at TEXT NOT NULL
);

CREATE TABLE c2_file_snapshots (
    session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES c2_tasks(id) ON DELETE CASCADE,
    schema_version TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    PRIMARY KEY(session_id, path)
);

CREATE TABLE c2_file_transfers (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES c2_tasks(id) ON DELETE SET NULL,
    direction TEXT NOT NULL CHECK(direction IN ('upload','download')),
    remote_path TEXT NOT NULL,
    storage_key TEXT NOT NULL UNIQUE,
    expected_size INTEGER,
    received_bytes INTEGER NOT NULL DEFAULT 0,
    sha256 TEXT,
    status TEXT NOT NULL CHECK(status IN ('queued','active','completed','error','cancelled')),
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);
```

- [ ] **Step 4: Add exact Rust model types**

Define the repository-facing models exactly as follows and derive `Debug`, `Clone`, `Serialize`, `Deserialize`, and `sqlx::FromRow`:

```rust
pub struct ProcessSnapshot {
    pub session_id: String,
    pub task_id: String,
    pub schema_version: String,
    #[sqlx(json)]
    pub snapshot_json: serde_json::Value,
    pub captured_at: String,
}

pub struct FileSnapshot {
    pub session_id: String,
    pub path: String,
    pub task_id: String,
    pub schema_version: String,
    #[sqlx(json)]
    pub snapshot_json: serde_json::Value,
    pub captured_at: String,
}

pub struct FileTransfer {
    pub id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub direction: String,
    pub remote_path: String,
    pub storage_key: String,
    pub expected_size: Option<i64>,
    pub received_bytes: i64,
    pub sha256: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}
```

Keep `capabilities_json` as a serialized `serde_json::Value` until the shared profile contract is introduced in Task 3.

- [ ] **Step 5: Run migrations twice and verify backward compatibility**

Run: `cargo test -p naughtywolf --test config_db_test`  
Expected: all configuration/migration tests PASS, including applying all migrations to a database containing pre-004 callbacks and completed task results.

- [ ] **Step 6: Commit schema and models**

```bash
git add migrations/004_callback_workspace.sql src/db/models.rs src/db/repositories.rs tests/config_db_test.rs
git commit -m "feat: add persistent callback workspace schema"
```

### Task 3: Fix Metadata Discovery and Callback Liveness

**Files:**
- Create: `crates/implant/src/metadata.rs`
- Modify: `crates/implant/src/lib.rs`
- Modify: `crates/implant/src/runtime.rs`
- Modify: `crates/implant/Cargo.toml`
- Create: `crates/profile/src/control.rs`
- Modify: `crates/profile/src/lib.rs`
- Modify: `crates/profile/src/msgs.rs`
- Modify: `src/c2.rs`
- Modify: `src/db/repositories.rs`
- Modify: `src/db/models.rs`
- Test: `crates/implant/src/metadata.rs`
- Test: `tests/repository_test.rs`

**Interfaces:**
- Produces: `metadata::discover(endpoint: &str, interval: Duration, jitter: Duration) -> HostMetadata`, shared `CallbackCapabilities`, and backward-compatible optional `Register` fields.
- Produces: `Callback::is_online(now: OffsetDateTime) -> bool` using `max(interval_ms * 3, 30_000)` milliseconds.

- [ ] **Step 1: Add a metadata test that removes hostname environment variables**

Serialize environment mutation in the test with the existing test process lock, temporarily remove `HOSTNAME` and `COMPUTERNAME`, call `metadata::discover`, restore both variables, and assert `hostname` is nonempty and not `unknown`.

- [ ] **Step 2: Run the metadata test and observe current discovery returning `unknown`**

Run: `cargo test -p nw-implant metadata::tests::discovers_hostname_without_exported_environment -- --nocapture`  
Expected: FAIL before `metadata.rs` exists or because hostname is `unknown`.

- [ ] **Step 3: Implement OS-backed discovery**

Add `hostname`, `whoami`, and `local-ip-address` dependencies. Create `nw_profile::control::CallbackCapabilities { process_browser: bool, file_browser: bool, file_transfer: bool, task_ack: bool }` with Serde derives and `Default` false for legacy callbacks. Define:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostMetadata {
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub os_version: Option<String>,
    pub arch: String,
    pub pid: u32,
    pub executable_path: Option<String>,
    pub local_addr: Option<String>,
    pub implant_version: String,
    pub interval_ms: u64,
    pub jitter_ms: u64,
    pub capabilities: CallbackCapabilities,
}
```

Use `hostname::get`, `whoami::username`, `std::env::consts`, `std::env::current_exe`, and `local_ip_address::local_ip`. Environment variables are fallback-only.

- [ ] **Step 4: Extend registration compatibly**

Add each new `Register` field as `Option<T>` with `#[serde(default)]`. Store values in both `callbacks` and `c2_sessions` where columns exist. Keep legacy deserialization coverage using the old JSON shape.

- [ ] **Step 5: Add deterministic liveness tests**

Test that a 1-second callback is online at 2 seconds and offline after 30 seconds, while a 20-second callback remains online through 60 seconds and becomes offline after 61 seconds.

- [ ] **Step 6: Run profile, implant, repository, and C2 tests**

Run: `cargo test -p nw-profile && cargo test -p nw-implant metadata && cargo test -p naughtywolf c2:: && cargo test -p naughtywolf --test repository_test`  
Expected: PASS with legacy and extended registrations.

- [ ] **Step 7: Commit metadata and liveness**

```bash
git add crates/profile/src/control.rs crates/profile/src/lib.rs crates/profile/src/msgs.rs crates/implant/Cargo.toml crates/implant/src/lib.rs crates/implant/src/metadata.rs crates/implant/src/runtime.rs src/c2.rs src/db/models.rs src/db/repositories.rs tests/repository_test.rs
git commit -m "feat: report trustworthy callback metadata"
```

### Task 4: Make Task Delivery Acknowledged, Idempotent, and Cancellable

**Files:**
- Modify: `crates/profile/src/msgs.rs`
- Modify: `crates/implant/src/runtime.rs`
- Modify: `src/c2.rs`
- Modify: `src/db/models.rs`
- Modify: `src/db/repositories.rs`
- Test: `src/c2.rs`
- Test: `crates/implant/tests/c2_end_to_end.rs`

**Interfaces:**
- Produces: `Repository::tasks_for_delivery(session_id)`, `Repository::acknowledge_tasks(session_id, ids)`, `Repository::request_task_cancellation(...)`, duplicate-safe implant acceptance, and acknowledged result delivery.
- Consumes: backward-compatible `PollRequest.accepted_task_ids`, `PollReply.result_acks`, and the existing `nw/killtask <task-uuid>` runtime command.

- [ ] **Step 1: Write a lost-poll-response test**

Enqueue one task, call the sealed poll processor twice without including its ID in `acked_ids`, and assert both replies contain the same database task UUID. Poll a third time with that ID acknowledged and assert the task is absent and status is `processing`.

- [ ] **Step 2: Run the test and observe the second reply is empty**

Run: `cargo test -p naughtywolf c2::tests::unacknowledged_task_is_redelivered_with_stable_id -- --nocapture`  
Expected: FAIL because `fetch_pending_tasks` excludes delivered rows.

- [ ] **Step 3: Implement delivery and acknowledgment transactions**

`tasks_for_delivery` selects `pending` plus unacknowledged `delivered` rows, preserves each task's UUID, and sets `delivered`/`updated_at`. `acknowledge_tasks` verifies the session owns every ID before setting `processing` and `processing_at`.

- [ ] **Step 4: Suppress duplicate execution in the implant**

Add `accepted_task_ids: Vec<Uuid>` to `PollRequest` and `result_acks: Vec<Uuid>` to `PollReply`, both with `#[serde(default)]`. Maintain bounded accepted/running/completed ID sets in the implant. Add newly accepted IDs to the next request; ignore a redelivered ID already running or awaiting result delivery. Do not `take()` pending results before exchange: resend them until their IDs appear in `result_acks`, then remove only those acknowledged results.

- [ ] **Step 5: Add cancellation race tests**

Cover cancellation while pending, cancellation while processing through `nw/killtask`, and normal completion winning before a cancellation request arrives. Assert one terminal audit outcome per target task.

- [ ] **Step 6: Run C2 end-to-end tests**

Run: `cargo test -p naughtywolf c2:: && cargo test -p nw-implant --test c2_end_to_end`  
Expected: PASS, including stable UUID and cancellation tests.

- [ ] **Step 7: Commit reliable lifecycle behavior**

```bash
git add crates/profile/src/msgs.rs crates/implant/src/runtime.rs src/c2.rs src/db/models.rs src/db/repositories.rs crates/implant/tests/c2_end_to_end.rs
git commit -m "feat: acknowledge and reconcile callback tasks"
```

### Task 5: Build the Persistent Tasking API and Tab

**Files:**
- Create: `src/callback_workspace/mod.rs`
- Create: `src/callback_workspace/tasks.rs`
- Create: `static/callback-workspace.js`
- Create: `static/callback-workspace.css`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Modify: `src/portal.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/admin.js`
- Test: `tests/callback_workspace_test.rs`
- Test: `tests/callback_workspace_test.cjs`
- Test: `tests/navigation_test.cjs`

**Interfaces:**
- Produces: paginated task DTOs and authenticated SSE snapshots from the API paths defined in the spec.
- Produces: `window.NWCallbackWorkspace.init(root)`; removes callback-specific task state from `static/admin.js` after parity tests pass.

- [ ] **Step 1: Add HTTP tests for pagination, error propagation, and access control**

Create 35 tasks with deterministic timestamps. Assert `limit=20` returns 20 newest tasks plus a `next_before` cursor, the second request returns 15 non-overlapping tasks, an out-of-scope operator receives 404, and a forced repository failure returns 500 rather than `{tasks:[]}`.

- [ ] **Step 2: Run the new HTTP tests and observe missing API routes**

Run: `cargo test -p naughtywolf --test callback_workspace_test task_api -- --nocapture`  
Expected: FAIL with 404 for `/api/callbacks/{id}/tasks`.

- [ ] **Step 3: Implement task DTO and cursor queries**

Define `TaskPage { tasks: Vec<TaskView>, next_before: Option<String> }`. Encode the cursor from `(created_at, id)` and query with `WHERE session_id=? AND (created_at,id) < (?,?) ORDER BY created_at DESC,id DESC LIMIT ?`.

Implement enqueue, retry, and cancel POST handlers in the same module. Each accepts the authenticated user's CSRF header, verifies callback visibility before task lookup, stores `operator_id`, and writes the audit event in the task transaction. Retry sets `parent_task_id` to the original task; cancel follows the lifecycle rules from Task 4.

- [ ] **Step 4: Implement reconciliation SSE**

Emit complete `TaskView` snapshots for changed tasks. On connection, emit the current page. Keep alive every 15 seconds. The JS client refetches the first page on `EventSource.onopen` after a reconnect and upserts by task ID.

- [ ] **Step 5: Write DOM behavior tests before the UI implementation**

In a Node test with the repository's DOM test harness, assert that two snapshots with the same task ID render one card, `pending` changes to `completed`, reload data restores prior output, filters hide without deleting, and ArrowUp selects the newest persisted command.

- [ ] **Step 6: Implement the Tasking tab**

Render task cards with command, state, operator, timestamps, arguments, stdout, stderr, exit code, retry, and cancel. Add search/state/error filters, load older, connection state, offline queue copy, and sticky command dock. Use `textContent` for all remote values.

- [ ] **Step 7: Run Rust and JavaScript tasking suites**

Run: `cargo test -p naughtywolf --test callback_workspace_test && node --test tests/callback_workspace_test.cjs tests/navigation_test.cjs && node --check static/callback-workspace.js`  
Expected: PASS with no duplicate cards or dropped history.

- [ ] **Step 8: Commit the tasking workspace**

```bash
git add src/callback_workspace src/lib.rs src/main.rs src/portal.rs src/portal/templates.rs static/admin.js static/callback-workspace.js static/callback-workspace.css tests/callback_workspace_test.rs tests/callback_workspace_test.cjs tests/navigation_test.cjs
git commit -m "feat: add persistent Mythic-style tasking"
```

### Task 6: Add Cross-Platform Process Control

**Files:**
- Modify: `crates/profile/src/control.rs`
- Modify: `crates/profile/src/lib.rs`
- Create: `crates/implant/src/processes.rs`
- Modify: `crates/implant/src/lib.rs`
- Modify: `crates/implant/src/runtime.rs`
- Modify: `crates/implant/Cargo.toml`
- Create: `src/callback_workspace/processes.rs`
- Modify: `src/callback_workspace/mod.rs`
- Modify: `src/db/repositories.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/callback-workspace.js`
- Modify: `static/callback-workspace.css`
- Test: `crates/implant/tests/callback_controls.rs`
- Test: `tests/callback_workspace_test.rs`

**Interfaces:**
- Produces: `ProcessListV1 { schema, captured_at, processes }`, `ProcessEntry`, and `ProcessKillV1`.
- Produces: `processes::list() -> ProcessListV1` and `processes::kill(pid: u32) -> Result<ProcessKillV1, ControlError>`.

- [ ] **Step 1: Define serialized contract tests**

Assert `ProcessListV1` serializes with schema `nw.process-list.v1`, nullable platform fields, numeric PID/parent PID, byte memory, CPU float, and RFC3339 capture time. Deserialize the same fixture on all targets.

- [ ] **Step 2: Add safe process integration tests**

Assert the current test PID appears in `processes::list`. Spawn a dedicated long-running child, call `processes::kill(child.id())`, and assert only that child exits. Never target an ambient PID.

- [ ] **Step 3: Run process tests and observe missing module/contracts**

Run: `cargo test -p nw-implant --test callback_controls process_ -- --nocapture`  
Expected: compile failure because `nw_profile::control` and `nw_implant::processes` do not exist.

- [ ] **Step 4: Implement process contracts and adapter**

Use `sysinfo` for process refresh, listing, parent relation, executable, user ID, CPU, memory, start time, and normal termination. Map `NoSuchProcess`, `PermissionDenied`, and `TerminateFailed` to stable `ControlError` codes.

- [ ] **Step 5: Dispatch structured process tasks and persist projections**

Handle only exact `nw/process-list` and `nw/process-kill` command names. Serialize successful results into task stdout. When storing a successful `nw.process-list.v1` result, validate its schema and atomically upsert `c2_process_snapshots` with the task result.

- [ ] **Step 6: Add API and UI tests**

Assert refresh/kill require CSRF and callback scope, kill audits the exact PID, stale snapshot time is visible, confirmation contains the PID and process name, and a successful kill queues an automatic refresh after completion.

- [ ] **Step 7: Implement the Processes tab**

Add sortable/searchable table, explicit refresh, snapshot timestamp, detail drawer, exact-target kill confirmation, offline/capability messages, and links from each generated control task to Tasking.

- [ ] **Step 8: Run cross-platform-safe suites and commit**

Run: `cargo test -p nw-profile control && cargo test -p nw-implant --test callback_controls process_ && cargo test -p naughtywolf --test callback_workspace_test process_`  
Expected: PASS.

```bash
git add crates/profile/src/control.rs crates/profile/src/lib.rs crates/implant/Cargo.toml crates/implant/src/lib.rs crates/implant/src/processes.rs crates/implant/src/runtime.rs src/callback_workspace/processes.rs src/callback_workspace/mod.rs src/db/repositories.rs src/portal/templates.rs static/callback-workspace.js static/callback-workspace.css crates/implant/tests/callback_controls.rs tests/callback_workspace_test.rs
git commit -m "feat: add audited callback process control"
```

### Task 7: Add Cross-Platform Filesystem Control

**Files:**
- Modify: `crates/profile/src/control.rs`
- Create: `crates/implant/src/filesystem.rs`
- Modify: `crates/implant/src/lib.rs`
- Modify: `crates/implant/src/runtime.rs`
- Create: `src/callback_workspace/files.rs`
- Modify: `src/callback_workspace/mod.rs`
- Modify: `src/db/repositories.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/callback-workspace.js`
- Modify: `static/callback-workspace.css`
- Test: `crates/implant/tests/callback_controls.rs`
- Test: `tests/callback_workspace_test.rs`

**Interfaces:**
- Produces: `FileListV1`, `FileEntry`, `FileMutationV1`, and typed `filesystem::{list, stat, mkdir, move_path, delete}` functions.
- Consumes: tasking, snapshot projection, audit, and confirmation primitives from Tasks 2–6.

- [ ] **Step 1: Add filesystem contract and temporary-directory tests**

Create a temporary tree containing a directory, UTF-8 filename, empty file, and nonempty file. Assert list/stat fields and absolute paths. Assert mkdir and move succeed. Assert nonrecursive delete refuses a nonempty directory and recursive delete removes only the exact temporary subtree.

- [ ] **Step 2: Run tests and observe missing typed filesystem module**

Run: `cargo test -p nw-implant --test callback_controls filesystem_ -- --nocapture`  
Expected: compile failure because `nw_implant::filesystem` does not exist.

- [ ] **Step 3: Implement versioned contracts and typed operations**

Use Rust filesystem APIs only. Reject empty paths and embedded NUL. Preserve Windows drive/UNC prefixes and Linux `/`. Return stable codes for not found, permission denied, already exists, nonempty directory, invalid path, and I/O failure.

- [ ] **Step 4: Dispatch and project filesystem tasks**

Implement exact commands from the spec. Validate argument count and boolean recursion before filesystem access. Successful list results atomically upsert `c2_file_snapshots` keyed by callback and normalized requested path.

- [ ] **Step 5: Add API security tests**

Assert every filesystem mutation requires operator role, callback visibility, CSRF, and exact path audit. Assert paths appear in escaped text, not HTML, and no route invokes a shell.

- [ ] **Step 6: Implement the Files tab without transfers**

Add URL-backed path navigation, breadcrumbs, parent navigation, sortable entries, refresh, mkdir, move/rename, delete, snapshot time, capability message, and exact-target confirmation. Render upload/download controls disabled with “Transfer support is being initialized” until Task 8.

- [ ] **Step 7: Run filesystem suites and commit**

Run: `cargo test -p nw-profile control && cargo test -p nw-implant --test callback_controls filesystem_ && cargo test -p naughtywolf --test callback_workspace_test filesystem_ && node --check static/callback-workspace.js`  
Expected: PASS.

```bash
git add crates/profile/src/control.rs crates/implant/src/lib.rs crates/implant/src/filesystem.rs crates/implant/src/runtime.rs src/callback_workspace/files.rs src/callback_workspace/mod.rs src/db/repositories.rs src/portal/templates.rs static/callback-workspace.js static/callback-workspace.css crates/implant/tests/callback_controls.rs tests/callback_workspace_test.rs
git commit -m "feat: add audited callback filesystem control"
```

### Task 8: Complete Durable Upload and Download Transfer

**Files:**
- Modify: `crates/profile/src/msgs.rs`
- Modify: `crates/implant/src/download.rs`
- Modify: `crates/implant/src/upload.rs`
- Modify: `crates/implant/src/runtime.rs`
- Create: `src/callback_workspace/transfers.rs`
- Modify: `src/callback_workspace/files.rs`
- Modify: `src/callback_workspace/mod.rs`
- Modify: `src/c2.rs`
- Modify: `src/config.rs`
- Modify: `src/db/repositories.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/callback-workspace.js`
- Test: `crates/implant/tests/c2_end_to_end.rs`
- Test: `tests/callback_workspace_test.rs`

**Interfaces:**
- Produces: transfer-bound `FileChunk { transfer_id, task_id, name, offset, total, data }` and `FileAck { transfer_id, received, total, done }` with serde defaults for legacy decoding.
- Produces: `TransferStore::receive_chunk`, `next_upload_chunks`, `ack_upload`, `publish_download`, and authenticated transfer endpoints.

- [ ] **Step 1: Add protocol compatibility tests**

Assert new chunks round-trip transfer/task UUIDs and old serialized chunks deserialize with absent IDs as `None`, which the server rejects for new transfers with a stable protocol error rather than panicking.

- [ ] **Step 2: Add transfer-store tests before implementation**

Cover contiguous chunks, duplicate chunks, gap rejection, resume from persisted offset, maximum-size rejection, SHA-256 mismatch, atomic `.part` to completed publication, and refusal to download active/error transfers.

- [ ] **Step 3: Run tests and observe missing TransferStore**

Run: `cargo test -p naughtywolf --test callback_workspace_test transfer_ -- --nocapture`  
Expected: compile failure because `callback_workspace::transfers::TransferStore` does not exist.

- [ ] **Step 4: Implement protected transfer storage**

Use generated storage keys beneath `${NAUGHTYWOLF_EVIDENCE_DIR}/callback-transfers`. Open files with create-new semantics, validate the resolved parent and regular-file type, cap size with a new `NAUGHTYWOLF_MAX_TRANSFER_BYTES` default of 268435456, fsync, verify SHA-256, and rename atomically on completion.

- [ ] **Step 5: Integrate poll chunks and acknowledgments**

In `process_sealed_poll`, persist every validated implant download chunk and return `FileAck`s. Use implant `upload_acks` to advance server-to-implant uploads and fill `PollReply.push_chunks` within `inner_budget`. Preserve one active upload and one active download per callback; leave later transfers queued.

- [ ] **Step 6: Add end-to-end transfer tests**

Run a native implant/server fixture that downloads a 3 KiB remote file and uploads a different 3 KiB file, force a reconnect midway, then assert resumed bytes and final SHA-256 equality in both directions.

- [ ] **Step 7: Implement authenticated upload/download UI**

Accept multipart staging upload with destination path, show progress from SSE, activate upload/download actions, show checksum and terminal error, and expose only completed verified downloads through the scoped endpoint.

- [ ] **Step 8: Run transfer, security, and C2 suites**

Run: `cargo test -p nw-profile && cargo test -p nw-implant download upload && cargo test -p nw-implant --test c2_end_to_end && cargo test -p naughtywolf --test callback_workspace_test transfer_`  
Expected: PASS including reconnect/resume and access-control cases.

- [ ] **Step 9: Commit durable transfers**

```bash
git add crates/profile/src/msgs.rs crates/implant/src/download.rs crates/implant/src/upload.rs crates/implant/src/runtime.rs src/callback_workspace/transfers.rs src/callback_workspace/files.rs src/callback_workspace/mod.rs src/c2.rs src/config.rs src/db/repositories.rs src/portal/templates.rs static/callback-workspace.js crates/implant/tests/c2_end_to_end.rs tests/callback_workspace_test.rs
git commit -m "feat: add resumable callback file transfers"
```

### Task 9: Finish Metadata Tab, Workspace UX, and Offline Supervision Guidance

**Files:**
- Modify: `src/portal/templates.rs`
- Modify: `static/callback-workspace.js`
- Modify: `static/callback-workspace.css`
- Modify: `README.md`
- Test: `tests/callback_workspace_test.rs`
- Test: `tests/navigation_test.cjs`

**Interfaces:**
- Consumes: authoritative task, metadata, process, filesystem, and transfer APIs.
- Produces: complete four-tab progressive workspace with URL-backed tab/path state and supervised implant guidance.

- [ ] **Step 1: Add complete workspace rendering tests**

Assert the callback page exposes Tasking, Processes, Files, and Metadata tabs; no secret/session key; correct online/offline copy; callback ID and safe metadata; URL state; keyboard focus; and disabled capability messaging for a legacy implant.

- [ ] **Step 2: Add navigation and confirmation tests**

Assert SPA navigation closes the prior EventSource, returning restores the selected tab/path, Escape closes a destructive dialog, Enter cannot activate the obscured page, and focus returns to the action button.

- [ ] **Step 3: Run UI tests and observe missing metadata, liveness, URL, and focus behavior**

Run: `cargo test -p naughtywolf --test callback_workspace_test workspace_ && node --test tests/navigation_test.cjs tests/callback_workspace_test.cjs`  
Expected: FAIL on missing metadata/offline/URL/focus behavior.

- [ ] **Step 4: Complete the workspace shell and responsive styling**

Add callback identity header, connection indicator, tablist semantics, responsive split/detail layouts, empty/loading/error states, metadata definition list, and consistent state/action styling. Preserve server-rendered fallback content when JavaScript is unavailable.

- [ ] **Step 5: Document why callbacks become offline and how to supervise implants**

Add an operator section explaining that closing the launching shell stops an unsupervised implant, show a narrowly scoped systemd unit template with an absolute payload path and unprivileged user, and document checking `last_seen`, PID, logs, and queued task state. Do not install or enable a service automatically.

- [ ] **Step 6: Run accessibility, syntax, and portal suites**

Run: `cargo test -p naughtywolf --test callback_workspace_test && cargo test -p naughtywolf --test portal_routes_test && node --test tests/navigation_test.cjs tests/callback_workspace_test.cjs && node --check static/callback-workspace.js`  
Expected: PASS.

- [ ] **Step 7: Commit final workspace UX and documentation**

```bash
git add src/portal/templates.rs static/callback-workspace.js static/callback-workspace.css README.md tests/callback_workspace_test.rs tests/navigation_test.cjs
git commit -m "feat: finish the callback operator workspace"
```

### Task 10: Add Windows CI, Full Verification, and VPS Rollout

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `.github/workflows/deploy-vps.yml`
- Modify: `Dockerfile`
- Modify: `tests/container_smoke.py`
- Create: `tests/workflow_test.py`
- Modify: `crates/server/src/creds.rs`
- Modify: `docs/superpowers/plans/2026-09-10-mythic-callback-workspace.md`

**Interfaces:**
- Consumes: all prior tasks.
- Produces: Linux and Windows CI evidence plus exact-SHA, health-checked VPS deployment.

- [ ] **Step 1: Add CI workflow assertions**

Create a repository test that parses workflow YAML and asserts jobs named `linux` and `windows`, Linux commands for `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, JavaScript tests, and Windows commands for profile/implant/control tests.

- [ ] **Step 2: Run the workflow test and observe missing `ci.yml`**

Run: `python3 tests/workflow_test.py`  
Expected: FAIL because `.github/workflows/ci.yml` does not exist.

- [ ] **Step 3: Implement Linux/Windows CI and deployment gating**

Use `ubuntu-latest` and `windows-latest`. Cache Cargo by lockfile. Make deployment run the Linux verification commands before SSH deployment so an untested push cannot replace the healthy VPS container. Rename the intentionally retained but unread `CredentialStore.pool` field to `_pool` and update its initializers so the existing dead-code warning does not defeat `-D warnings`.

- [ ] **Step 4: Run fresh local verification**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --test tests/*.cjs
node --check static/callback-workspace.js
python3 tests/workflow_test.py
bash tests/vps_deploy_test.sh
```

Expected: every command exits 0 with zero test failures.

- [ ] **Step 5: Build and smoke-test the production image**

Run the repository's container smoke command used by CI and assert migrations apply, `/healthz` succeeds, static workspace assets return JavaScript/CSS content types, and pre-004 task history remains readable.

- [ ] **Step 6: Commit CI and verification changes**

```bash
git add .github/workflows/ci.yml .github/workflows/deploy-vps.yml Dockerfile tests/container_smoke.py tests/workflow_test.py docs/superpowers/plans/2026-09-10-mythic-callback-workspace.md
git commit -m "ci: verify callback workspace on Linux and Windows"
```

- [ ] **Step 7: Push `develop` and obtain both workflow IDs**

Run: `git push origin develop`, then `gh run list --branch develop --limit 4 --json databaseId,workflowName,headSha,status,url`.  
Expected: output contains CI and deployment runs whose `headSha` equals local `git rev-parse HEAD`.

- [ ] **Step 8: Monitor the exact numeric IDs printed by Step 7**

Run `gh run watch` once for each numeric `databaseId` printed by Step 7, passing `--exit-status`.  
Expected: CI succeeds before the deployment job reports success.

- [ ] **Step 9: Verify the deployed production state**

Over the authorized SSH connection, assert `/opt/naughtywolf` resolves to the pushed commit, `naughtywolf-app-1` is `running healthy`, and migrations include version 004. Assert `https://gateofbabylon.space/` and `/login` return 200.

- [ ] **Step 10: Perform an authorized non-destructive callback smoke**

Start a freshly built supervised Linux test implant, wait for a real hostname and advancing `last_seen`, queue `nw/process-list` and `nw/fs-list /tmp`, and assert both transition pending → delivered → processing → completed, persist after page reload, and populate their tabs. Do not invoke kill/delete during deployment smoke.

- [ ] **Step 11: Mark the plan complete only after evidence is recorded**

Check every task box only when its named command has fresh exit-0 output. Record the final commit SHA, CI URLs, container health, callback ID, and the two non-destructive smoke task IDs in the implementation handoff.
