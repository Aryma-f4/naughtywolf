# Transfer Runtime Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the five load-bearing Task 8 gaps with stable filesystem capabilities, full-lifetime cancellation, exact wire/body limits, and a production-stack restart test.

**Architecture:** Replace timeout leases as the exclusion boundary with a generated per-transfer OS lock file held for the entire write/hash/fsync/publish operation. Model implant transfers as asynchronous jobs whose completion channel stays registered until publication or cancellation. Extract the production HTTP router composition so a file-backed integration test can drive the real portal and C2 routes with a real `BeaconRuntime`, sever the socket, reopen SQLite, reconstruct `TransferStore`, and resume.

**Tech Stack:** Rust 2024, Tokio, Axum, SQLx/SQLite, `libc`, `windows-sys`, SHA-256, vanilla JavaScript tests.

**Spec:** `docs/superpowers/specs/2026-09-10-mythic-callback-workspace-design.md`

## Global Constraints

- Linux and Windows are first-class targets; no shell is used for filesystem transfer operations.
- A user-provided remote path is never joined into server evidence storage; storage components are generated IDs only.
- Upload publication is create-without-replace and never exposes partial bytes at the final path.
- One upload and one download may be active per callback; later transfers remain FIFO by `(created_at,id)`.
- Result, task, transfer, audit, and projection state changes remain transactionally consistent.
- All transfer bodies and sealed C2 frames obey configured/transport byte limits.
- Tests mutate only temporary files, temporary SQLite databases, and child runtimes created by the test.
- Preserve all accepted Task 1–7 behavior and public Task 8 serde compatibility.

---

### Task 1: Make Transfer Storage Capability-Safe and Cross-Process Exclusive

**Files:**
- Modify: `src/callback_workspace/transfers.rs`
- Modify: `crates/implant/src/upload.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/implant/Cargo.toml`
- Test: `tests/callback_workspace_test.rs`
- Test: `crates/implant/src/upload.rs`

**Interfaces:**
- Produces: `TransferOperationLock`, acquired before file mutation and held through write, fsync, hashing, link publication, and directory fsync.
- Produces: Windows ancestor guards that retain non-delete-sharing handles until the final handle-based operation completes.
- Consumes: existing generated `storage_key`, held Unix `RootHandle`, and transfer-specific implant sidecar names.

- [ ] **Step 1: Add failing cross-instance and Windows/Unix path-race tests**

Add `cross_store_lock_precedes_part_write`, `unix_cleanup_closes_directory_on_invalid_name`, and platform-gated `windows_guards_reparse_ancestors_until_publication`. The cross-store test creates two `TransferStore` instances over one file-backed DB/root, externally locks `<storage_key>.lock`, calls `receive_chunk` through the second store, and asserts the part bytes and DB offset remain unchanged.

For implant upload, add `preexisting_or_reparse_destination_is_never_modified` and assert the transfer-specific `.nwpart-<uuid>` is the only path containing partial bytes.

- [ ] **Step 2: Run the new tests and capture RED**

Run:

```bash
cargo test -p naughtywolf --test callback_workspace_test cross_store_lock_precedes_part_write unix_cleanup_closes_directory_on_invalid_name -- --nocapture
cargo test -p nw-implant upload::tests -- --nocapture
```

Expected: cross-store mutation proceeds despite the external lock; the Unix invalid-name path leaks the duplicated directory descriptor; Windows source lacks a retained ancestor guard.

- [ ] **Step 3: Implement `TransferOperationLock`**

Use a generated `<storage_key>.lock` component under the held root. On Unix open it with `openat(O_CREAT|O_RDWR|O_NOFOLLOW|O_CLOEXEC, 0600)` and hold `flock(LOCK_EX|LOCK_NB)` for the whole operation. On Windows open it with `CreateFileW` and acquire `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY)`. `Drop` releases the OS lock; process death also releases it. Acquire this guard before opening or modifying `.part`, before DB offset CAS, and before hash/publication/reconciliation.

Keep the SQLite lease only as observable ownership metadata. Its expiry must never permit a second writer while the OS lock is held.

- [ ] **Step 4: Retain Windows ancestor handles and use handle-safe deletion**

Replace validate-and-close helpers with a `WindowsPathGuard { handles: Vec<File>, parent: File, path: PathBuf }`. Open every ancestor with `FILE_FLAG_OPEN_REPARSE_POINT|FILE_FLAG_BACKUP_SEMANTICS`, omit `FILE_SHARE_DELETE`, reject `FILE_ATTRIBUTE_REPARSE_POINT`, and retain all handles through `CreateHardLinkW`.

Delete cleanup targets by an opened target handle using `SetFileInformationByHandle(FileDispositionInfo)` rather than reopening the pathname. The server root contains only one generated component below the guarded root. Apply the same retained-ancestor pattern to implant destination parents and sidecar publication.

- [ ] **Step 5: Fix Unix directory enumeration cleanup**

Wrap the `DIR*` returned by `fdopendir` in a small RAII guard whose `Drop` always calls `closedir`. Set `errno=0` before `readdir`; distinguish EOF (`errno==0`) from error. Reject non-UTF-8 generated storage entries without leaking the duplicated descriptor.

- [ ] **Step 6: Verify storage tests and commit**

Run:

```bash
cargo test -p naughtywolf --test callback_workspace_test transfer_ -- --nocapture
cargo test -p nw-implant upload::tests -- --nocapture
cargo fmt --all -- --check
```

Expected: all pass and cross-store attempts cannot write before obtaining the OS lock.

```bash
git add Cargo.toml Cargo.lock crates/implant/Cargo.toml crates/implant/src/upload.rs src/callback_workspace/transfers.rs tests/callback_workspace_test.rs
git commit -m "fix: make transfer storage capability safe"
```

### Task 2: Keep Transfer Jobs Cancellable and Enforce Actual Byte Budgets

**Files:**
- Modify: `crates/implant/src/runtime.rs`
- Modify: `crates/implant/src/download.rs`
- Modify: `crates/implant/src/upload.rs`
- Modify: `src/c2.rs`
- Modify: `src/callback_workspace/mod.rs`
- Modify: `src/portal.rs`
- Modify: `src/db/repositories.rs`
- Test: `crates/implant/src/runtime.rs`
- Test: `src/c2.rs`
- Test: `tests/callback_workspace_test.rs`

**Interfaces:**
- Produces: `ActiveUpload` and `ActiveDownload` runtime slots containing the transfer object, a cancellation flag, and a one-shot completion sender.
- Produces: `fit_poll_request_to_budget` and `fit_poll_reply_to_budget`, which return only envelopes whose actual sealed wire length is at most `inner_budget`.
- Consumes: Task 1's sidecar publication and OS lock; current repository terminal-state transaction.

- [ ] **Step 1: Add failing lifecycle tests**

Add `kill_during_upload_finalize_yields_one_cancelled_result`, using a finalization barrier after hashing starts and before publication. Issue `nw/killtask` only after the barrier, release it, and assert: final destination absent, sidecar cleaned, exactly one cancelled result, no success result, transfer `cancelled`, and the next same-direction FIFO task becomes deliverable.

Add the equivalent active download cancellation test. The production change these tests catch is removing the runtime slot/task registration before finalization completes.

- [ ] **Step 2: Add failing actual-wire and configured-body tests**

Add a C2 test that calls the real sealed check-in handler and asserts the returned response body length is `<= inner_budget` for: non-transfer-only payload, one chunk that requires truncation, and a fixed payload too large to fit. Add a runtime request test that inspects the actual sealed request bytes, not plaintext JSON.

Build the HTTP test app with `portal::authenticated_router_with_transfer_limit(4096)`. Assert a 3 KiB multipart succeeds, a 5 KiB multipart returns `413 Payload Too Large`, and neither case relies on the default 256 MiB router.

- [ ] **Step 3: Run lifecycle and budget tests and capture RED**

Run:

```bash
cargo test -p nw-implant runtime::tests::kill_during_ -- --nocapture
cargo test -p naughtywolf c2::tests::sealed_wire_ -- --nocapture
cargo test -p naughtywolf --test callback_workspace_test configured_multipart_limit -- --nocapture
```

Expected: finalize escapes cancellation; the request/reply helper either rejects or emits zero progress near DNS budget; multipart test uses the wrong router limit.

- [ ] **Step 4: Make transfer completion own runtime task lifetime**

Introduce runtime-only `ActiveUpload`/`ActiveDownload` wrappers. `run_one` installs a slot and awaits its one-shot receiver; it does not return merely because `upload.take()` occurred. Poll processing keeps the slot installed with a `Finalizing` phase while hashing/publishing. It sends exactly one terminal `TaskResult`, then removes the slot and resolves the receiver.

`kill_task` sets the slot cancellation flag and takes the slot through one transition. Finalization checks the flag before publication. Cancellation removes the sidecar/part, sends one cancelled result, and lets the repository's existing transaction mark task and transfer cancelled and advance FIFO.

- [ ] **Step 5: Fit the actual sealed wire in both directions**

Construct candidate `PollRequest`/`PollReply`, encrypt and `Envelope::seal`, then measure the returned wire bytes. Reserve fixed non-transfer payload first. If a file chunk does not fit, binary-search/truncate `FileChunk.data` while keeping its offset/total; never discard the only chunk when at least one byte can fit. If fixed payload cannot fit, return a stable budget error and do not transmit an oversized frame.

- [ ] **Step 6: Wire the configured multipart router explicitly**

Keep `DefaultBodyLimit::max(max_transfer_bytes + 64 KiB)` on only the multipart route, use checked `usize` conversion, and ensure every production/test application builder receives the configured value instead of calling `authenticated_router()` implicitly.

- [ ] **Step 7: Verify lifecycle/budget suites and commit**

Run:

```bash
cargo test -p nw-implant runtime::tests -- --nocapture
cargo test -p naughtywolf c2::tests -- --nocapture
cargo test -p naughtywolf --test callback_workspace_test configured_multipart_limit -- --nocapture
cargo test -p naughtywolf --test callback_workspace_test transfer_ -- --nocapture
```

Expected: all pass; actual sealed request/reply bytes fit and finalization remains cancellable.

```bash
git add crates/implant/src/runtime.rs crates/implant/src/download.rs crates/implant/src/upload.rs src/c2.rs src/callback_workspace/mod.rs src/portal.rs src/db/repositories.rs tests/callback_workspace_test.rs
git commit -m "fix: keep transfer jobs cancellable"
```

### Task 3: Prove Restart and Resume Through the Production Application

**Files:**
- Create: `src/application.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Create: `tests/transfer_production_e2e.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces: `application::router(repository, c2_psk, evidence_store, transfer_store, max_transfer_bytes) -> Router`, containing the same portal, C2, public, and extension composition used by `serve()` before the session layer.
- Consumes: Task 1 storage/exclusion and Task 2 runtime lifecycle/wire budgeting.

- [x] **Step 1: Write the production-stack test first**

Create a temporary file-backed SQLite database and evidence directory. Build the application with the exported production router, attach a test session layer/login route, bind a real TCP listener, and spawn a real `BeaconRuntime` pointed at `/c2/checkin`.

The test must:

1. Register the implant through `/c2/checkin`, create an operator scope, and stage a distinct 3 KiB server-to-implant upload through the authenticated multipart route.
2. Queue a distinct 3 KiB implant-to-server download through the authenticated files route.
3. Queue same-direction FIFO followers before either predecessor completes and force equal `created_at` values with inverse ID/row insertion order.
4. Use middleware that blocks a partial C2 response after both durable offsets are between `1` and `3071`; signal the test, abort the listener, and prove the client receives a real socket error.
5. Stop the first app, close the pool, reopen the same SQLite file, reconstruct `Repository`, `TransferStore`, session layer, and application router, and rebind the same endpoint.
6. Let the same implant runtime reconnect; do not call `TransferStore::{receive_chunk,next_upload_chunks,ack_upload}` directly from the test.
7. Assert both final files equal their hand-built 3 KiB fixtures and SHA-256 values; transfers/tasks are terminal; result ACKs converge; partial files are gone; followers are delivered only afterward.

- [x] **Step 2: Run the new test and capture RED**

Run:

```bash
cargo test -p naughtywolf --test transfer_production_e2e -- --nocapture
```

Expected: compile failure because `application::router` does not exist, followed by behavioral failures until the test drives real C2 chunks and reconnects after pool/store reconstruction.

- [x] **Step 3: Extract production router composition without changing behavior**

Move only the router composition from `main::serve` into `src/application.rs`. `main` remains responsible for config loading, migrations, production `SqliteStore`, secure cookie key, listener binding, and background raw TCP listener. The test and `main` must call the same `application::router` function.

- [x] **Step 4: Implement condition-based restart orchestration**

Use `Notify`/watch channels tied to observed durable offsets; no fixed sleep decides when to restart. Hold a response after the real C2 handler has persisted partial progress, abort the Axum server, close/reopen the database, rebuild dependencies, and release the implant to retry against the reconstructed server.

- [x] **Step 5: Run integration and full verification**

Run:

```bash
cargo test -p naughtywolf --test transfer_production_e2e -- --nocapture
cargo test --workspace
node --test tests/callback_workspace_test.cjs tests/navigation_test.cjs
node --check static/callback-workspace.js
cargo fmt --all -- --check
git diff --check
```

Expected: all pass. The production E2E must report actual partial offsets before restart and terminal byte/hash equality after reconnect.

- [x] **Step 6: Commit production-stack proof**

```bash
git add Cargo.toml src/application.rs src/lib.rs src/main.rs tests/transfer_production_e2e.rs
git commit -m "test: prove production transfer restart recovery"
```
