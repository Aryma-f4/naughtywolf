# Task 4 Report — Acknowledged, Idempotent, Cancellable Task Delivery

## Status

Complete.

## TDD evidence

### RED — lost poll response

Command:

```text
cargo test -p naughtywolf c2::tests::unacknowledged_task_is_redelivered_with_stable_id -- --nocapture
```

Observed exit code `101`:

```text
thread 'c2::tests::unacknowledged_task_is_redelivered_with_stable_id' panicked at src/c2.rs:507:9:
assertion `left == right` failed
  left: 0
 right: 1
test c2::tests::unacknowledged_task_is_redelivered_with_stable_id ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 33 filtered out
```

This proved the second poll lost the already-delivered task.

### GREEN — stable delivery and lifecycle coverage

Command:

```text
cargo test -p naughtywolf c2:: -- --nocapture
```

Observed:

```text
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 30 filtered out
```

After adding the ownership regression, the final required run was:

```text
cargo test -p naughtywolf c2:: && cargo test -p nw-implant --test c2_end_to_end
```

Observed:

```text
running 8 tests
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 30 filtered out
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The new implant E2E test passed and proved one execution across duplicate delivery, result retry before ack, and removal only after `result_acks`. The first sandboxed attempt could not bind loopback (`PermissionDenied`); the same command passed with the required local-network permission.

Additional compatibility checks:

```text
cargo test -p nw-profile
test result: ok. 21 passed; 0 failed

cargo test -p naughtywolf --test repository_test -- --nocapture
test result: ok. 21 passed; 0 failed
```

## Files changed

- `crates/profile/src/msgs.rs`
- `crates/implant/src/runtime.rs`
- `crates/implant/tests/c2_end_to_end.rs`
- `src/c2.rs`
- `src/db/models.rs`
- `src/db/repositories.rs`
- `crates/server/src/channels.rs` (minimal compatibility update required by the extended `PollReply` wire shape)

## Behavior implemented

- Durable `pending|delivered` redelivery with stable database UUIDs.
- Atomic, all-or-nothing, session-owned task acknowledgement to `processing`.
- Backward-compatible `accepted_task_ids` and `result_acks` serde fields; legacy `acked_ids` remains accepted.
- Bounded implant task-ID memory, duplicate execution suppression, and results retained until explicit server acknowledgement.
- Session-owned, idempotent result storage with exactly one terminal audit outcome.
- Immediate queued cancellation, one idempotent `nw/killtask` child for accepted work, and completion-wins race semantics.

## Self-review

- Ran `rustfmt --check` on every changed Rust file and `git diff --check`: clean.
- Verified acknowledgement never downgrades terminal tasks and rejects a mixed owned/foreign ID batch without partial updates.
- Verified duplicate results are acknowledged without replacing immutable result bytes or duplicating terminal audit rows.
- Verified cancellation reuses the existing `nw/killtask <task-uuid>` path.

## Concerns

No functional blocker. Existing unrelated `nw-server::CredentialStore.pool` dead-code warning remains and is planned for Task 10.

## Fix round 1

### RED — serial accepted batch

Command:

```text
cargo test -p nw-implant runtime::tests::accepted_batch_runs_independently_and_kill_suppresses_queued_work -- --nocapture
```

Observed exit code `101`:

```text
thread 'runtime::tests::accepted_batch_runs_independently_and_kill_suppresses_queued_work' panicked at crates/implant/src/runtime.rs:789:10:
kill task must not wait behind the long-running victim: Elapsed(())
test runtime::tests::accepted_batch_runs_independently_and_kill_suppresses_queued_work ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 34 filtered out; finished in 2.02s
```

This reproduced the review finding: `nw/killtask` waited behind the long-running accepted task.

### GREEN — independent scheduling and accepted-before-start cancellation

Commands and observed results:

```text
cargo test -p nw-implant runtime::tests::accepted_batch_runs_independently_and_kill_suppresses_queued_work -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 34 filtered out

cargo test -p nw-implant runtime::tests::killtask_cancels_an_accepted_task_before_it_is_scheduled -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out

cargo test -p nw-implant runtime::tests::task_id_tracking_is_bounded -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out
```

The runtime now registers every accepted task in one active state map, schedules every task independently, records each abort handle before yielding, suppresses accepted-but-not-started work deterministically, and independently cleans completed task state. New acceptance is capped at 4096 active IDs; completed dedupe history evicts oldest IDs at the same bound.

### GREEN — real lossy poll cancellation lifecycle

Command:

```text
cargo test -p naughtywolf c2::tests::lost_delivery_then_processing_cancel_terminates_implant_task_once -- --nocapture
```

Observed:

```text
test c2::tests::lost_delivery_then_processing_cancel_terminates_implant_task_once ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 38 filtered out; finished in 0.24s
```

This test runs a real implant against the SQLite-backed portal poll processor, deliberately drops the first task-bearing HTTP response after the database records delivery, requests cancellation, then proves the redelivered long-running task and `nw/killtask` control execute concurrently. It asserts the victim reaches persisted `cancelled`, stores the real `task cancelled` result with exit `-1`, and has exactly one terminal audit event.

### Minor review coverage

- The stable-redelivery test seeds an obsolete delivery timestamp and proves acknowledgement overwrites `processing_at` with actual acceptance time.
- The JSON `/c2/poll` test now decodes `PollReply`, asserts the wire UUID equals the database UUID, acknowledges it, sends a result, verifies `result_acks`, and proves `processing_at` was populated.
- `task_id_tracking_is_bounded` explicitly proves the 4096 active acceptance cap and 4096-entry completed-ID eviction.

### Final GREEN suites

Command:

```text
cargo test -p naughtywolf c2:: && cargo test -p nw-implant --test c2_end_to_end && cargo test -p nw-implant --lib
```

Observed:

```text
running 9 tests
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 30 filtered out
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 37 tests
test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### Fix-round files

- `Cargo.toml`, `Cargo.lock` — root test-only dependency on `nw-implant` for the real portal/implant lifecycle test.
- `crates/implant/src/runtime.rs` — independently monitored task scheduling and atomic accepted/running/cancelled state.
- `src/c2.rs` — lossy-response cancellation integration test, JSON poll assertions, bounds/race coverage.
- `src/db/repositories.rs` — acceptance time always replaces a stale delivery timestamp.

### Fix-round self-review

- Each task monitor finalizes state when its own work finishes, independent of batch order.
- The active-state mutex makes accepted-to-running and cancellation decisions atomic; a control task can either suppress unstarted work or abort the registered outer task future, whose dropped child uses `kill_on_drop`.
- No task state is downgraded, no synthetic result is used in the new end-to-end cancellation proof, and duplicate result/audit behavior remains unchanged.
- `rustfmt --check` on changed Rust files and `git diff --check` are clean.

## Fix round 2

### RED — cancellation lacked a real process-start barrier

Command (run with loopback permission after the sandbox-only bind attempt was denied):

```text
cargo test -p naughtywolf c2::tests::lost_delivery_then_processing_cancel_terminates_implant_task_once -- --nocapture
```

Observed exit code `101`:

```text
thread 'c2::tests::lost_delivery_then_processing_cancel_terminates_implant_task_once' panicked at src/c2.rs:780:10:
implant must acknowledge and start the real child before cancellation: Elapsed(())
test c2::tests::lost_delivery_then_processing_cancel_terminates_implant_task_once ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 38 filtered out; finished in 5.10s
```

The incomplete barrier correctly failed because the delivered command was still `sleep 30` and could never create the `started` sentinel.

### GREEN — already-running child termination and portable commands

Focused real lifecycle command:

```text
cargo test -p naughtywolf c2::tests::lost_delivery_then_processing_cancel_terminates_implant_task_once -- --nocapture
```

Observed:

```text
test c2::tests::lost_delivery_then_processing_cancel_terminates_implant_task_once ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 38 filtered out; finished in 4.39s
```

The victim now creates `started`, waits three seconds, and would create `finished` only if it survived. The test waits for both persisted `processing` and `started` before requesting cancellation, retains the persisted `cancelled`, real `task cancelled` result, and exactly-one terminal audit assertions, then waits four seconds and proves `finished` is absent.

Focused amended runtime tests:

```text
cargo test -p nw-implant runtime::tests:: -- --nocapture
```

Observed:

```text
running 8 tests
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 29 filtered out; finished in 0.03s
```

Windows cross-target compile commands:

```text
env RUSTC=/Users/dsi/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/rustc /Users/dsi/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/cargo check -p nw-implant --tests --target x86_64-pc-windows-gnu && env RUSTC=/Users/dsi/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/rustc /Users/dsi/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/cargo check -p naughtywolf --tests --target x86_64-pc-windows-gnu
```

Observed exit code `0`:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.81s
Checking naughtywolf v0.1.0 (/Users/dsi/projects/naughtywolf)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.47s
```

The executable test commands use `sh` plus `sleep` on Unix and `cmd.exe` plus loopback `ping` on Windows. Existing warnings in `CredentialStore.pool`, `parse_passwd`, and Windows-only evidence permission stubs remain unrelated.

### Final GREEN suites

Command:

```text
cargo test -p naughtywolf c2:: && cargo test -p nw-implant --test c2_end_to_end && cargo test -p nw-implant --lib
```

Observed:

```text
running 9 tests
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 30 filtered out; finished in 4.50s
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.78s
running 37 tests
test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.02s
```

### Fix-round 2 self-review

- The start sentinel is written by the real OS child, after implant acceptance and before cancellation; database `processing` alone is no longer treated as proof of process start.
- Removing the runtime abort or allowing the child to continue through its wait makes the `finished` assertion fail, so this is distinct from accepted-before-start suppression.
- Platform-specific helpers keep the new executable cancellation tests buildable on Linux and Windows without changing production behavior.
- Scoped `rustfmt --check` and `git diff --check` pass; the only changed source files are `crates/implant/src/runtime.rs` and `src/c2.rs`.
