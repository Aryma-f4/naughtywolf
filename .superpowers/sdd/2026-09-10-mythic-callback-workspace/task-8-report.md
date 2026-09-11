# Task 8 report

- Inherited state: partial Task 8 implementation was already present at HEAD 45604e3 (23 modified files plus `transfers.rs`, about 1.16k pre-existing changed lines); all changes were preserved.
- RED: workspace verification first failed only at `nw-server::dispatch::tests::download_queues_nw_download_task`, whose assertion still expected legacy one-argument downloads.
- GREEN: updated that assertion to require the remote path plus a parseable transfer UUID; `cargo test --workspace` then passed.
- Transfer-store proof: `cargo test -p naughtywolf --test callback_workspace_test transfer_ -- --nocapture` passed 8/8.
- Protocol/implant proof: `cargo test -p nw-profile` passed 26/26; implant download/upload unit filters passed; elevated `cargo test -p nw-implant --test c2_end_to_end` passed 12/12 including forced reconnect/resume.
- UI proof: `node --test tests/callback_workspace_test.cjs` passed 22/22.
- Regression proof: final `cargo test --workspace` passed all workspace suites (including callback 33/33 and nw-server 30/30).
- Scoped files include protocol, implant transfer state, durable `TransferStore`, repository/C2 wiring, config, authenticated file endpoints, SSE/UI, multipart tests, and the required server compatibility changes.
- Extra `crates/server/*` changes are retained because the native implant HTTP fixture and legacy C2 transport use that crate; they carry transfer/task IDs through the compatibility path.
- Concern: `cargo fmt --all -- --check` still flags unrelated pre-existing formatting in `crates/implant/src/processes.rs` and `tests/config_db_test.rs`; no Task 1–7 behavior was changed for that.

## Fix round 1

- RED: added queue/cancellation regressions; before the fix, both queued directions were delivered together and cancellation left the transfer queued.
- GREEN: `cargo test -p naughtywolf --test callback_workspace_test transfer_ -- --nocapture` passed 10/10; cancellation regression passed 1/1.
- Terminal upload ACKs now remain durable in implant memory through reconnect-safe retransmission; the destination is published only after the server's terminal ACK confirmation. Production HTTP compatibility E2E passed with a forced 502 reconnect.
- TransferStore now uses checked offsets and CAS row counts, shared per-store I/O serialization, no-replace hard-link publication, parent-directory fsync, active-artifact reconciliation, bounded completed-artifact verification, upload sidecars bound to transfer IDs, startup orphan cleanup, and DTOs that omit `storage_key`.
- `cargo check -p naughtywolf -p nw-implant -p nw-profile` passed; focused `c2` frame-budget test passed; elevated `cargo test -p nw-implant --test c2_end_to_end upload_streams_a_local_file_to_the_implant -- --nocapture` passed with the forced 502.
- Remaining compatibility note: the legacy in-memory `nw-server` fixture remains only for existing transport tests; native production C2 uses SQLite-backed `TransferStore` through the application extension.
- Final verification: `cargo test --workspace` passed (40+3 naughtywolf, callback 35, config 6, portal 61, repository 21, implant 38 + 12 E2E, profile 26, server 30, modules 6); `node --test tests/callback_workspace_test.cjs` passed 22/22; `cargo fmt --all -- --check` and `git diff --check` passed.
- Reconnect hardening adds transfer-total task metadata and transfer-specific implant sidecars so a lost terminal response can be reconstructed; FIFO ties use SQLite rowid ordering. Final local commit: `659730404ae54578b7f0698b2aeb0e3c974ef4e1`.

## Fix round 2

- RED: review identified activation using `(created_at,id)` while delivery gating used SQLite rowid; this could deadlock equal-timestamp inverse ordering. GREEN: both paths now use `(created_at,id)` and keyed transfer locks; `cargo test -p naughtywolf --test callback_workspace_test transfer_ -- --nocapture` passed 10/10.
- Storage fixes use the platform `libc::O_NOFOLLOW` constant, no-follow regular-file opens, same-handle bounded download verification, no-replace publication, and transfer-specific sidecar staging.
- Frame validation now checks the complete sealed envelope including non-transfer payload even when no chunks fit; focused C2 budget test passed.
- Focused implant reconnect test passed: `cargo test -p nw-implant --test c2_end_to_end upload_streams_a_local_file_to_the_implant`; forced 502 was observed and recovered.
- Final Round 2 verification: `cargo test --workspace` passed all workspace suites (40+3 core, callback 35, portal 61, repository 21, implant 38 + E2E 12, profile 26, server 30, modules 6); `node --test tests/callback_workspace_test.cjs` passed 22/22; format and diff checks passed.
- Final local commit after Round 2: `93bb00de8375f82861a95204123a9cb05db14498`.

## Fix round 3

- RED: atomic FIFO review exposed a select/update race and inconsistent tie ordering. GREEN: `tasks_for_delivery` now atomically claims pending rows with SQLite `UPDATE ... RETURNING`, verifies the claim predicate, and activation uses the same `(created_at,id)` predecessor rule; transfer FIFO and C2 frame tests pass.
- Added route-level `DefaultBodyLimit` derived from configured max plus bounded multipart overhead, platform `libc::O_NOFOLLOW`, same-handle streaming verification, keyed locks, sidecar expected size/SHA metadata, and retained completed downloads during orphan cleanup.
- Focused verification passed: callback transfer tests 10/10, C2 budget test 1/1, implant upload reconnect E2E with forced 502, formatting/diff checks. Full workspace suite was rerun successfully before the final report amend.
- Final local commit after Round 3: recorded by `git rev-parse HEAD` at handoff (report-only amendment intentionally avoids a self-referential hash).

## Fix round 4

- RED regressions: final receiver chunks incorrectly marked transfers `completed` before receiver publication; separate `TransferStore` instances raced on the same transfer; ancestor replacement could redirect path-based storage; implant upload exposed the final destination progressively and could replace a preexisting file; terminal artifacts had no configurable expiry. Each regression failed before its corresponding fix.
- GREEN fixes: receiver completion remains provisional (`active`) until the authenticated task result confirms publication; `c2_transfer_leases` provides a short-lived SQLite cross-instance lease; Unix storage uses a held directory fd plus `openat`/`linkat`/`unlinkat` with `O_NOFOLLOW` and descriptor metadata checks; implant upload writes only `<destination>.nwpart-<transfer-id>` and uses atomic no-replace hard-link publication; terminal retention is configurable through `NAUGHTYWOLF_TRANSFER_RETENTION_SECS` (default 7 days), with timestamp-based expiry and orphan cleanup; keyed lock entries use `Weak` references and are evicted normally.
- Focused evidence: `cargo test -p naughtywolf --test callback_workspace_test transfer_ -- --nocapture` passed 10/10; full callback suite passed 39/39; new provisional, cross-instance, retention, and ancestor-swap regressions passed; `cargo test -p nw-implant download -- --nocapture` passed including forced 502 reconnect; `cargo test -p nw-implant upload::tests -- --nocapture` passed 2/2; `cargo test -p nw-profile` passed 26/26; `node --test tests/callback_workspace_test.cjs` passed 22/22; `cargo test --workspace` passed all workspace suites including 12/12 implant C2 E2E; `cargo fmt --all -- --check` and `git diff --check` passed.
- The existing implant C2 integration fixture remains backed by the legacy `nw-server` in-memory transfer stores; a new file-backed SQLite production-application HTTP fixture/reopen test is not added in this round and remains a required follow-up rather than being claimed as covered.
