# M2 Implementation Checklist — NaughtyWolf C2 Framework

Reference: `docs/superpowers/plans/2026-08-29-c2-framework-roadmap.md`
Milestone M2: "Operator model + payload generation"

This document cross-references the M2 roadmap spec against the actual code in
`crates/` and `src/c2.rs`. All items below were verified against the source
tree and passing tests (199 tests, 0 failures).

---

## Summary

M2 is **fully implemented** across two code tracks:
1. **New modular track** (`crates/profile`, `crates/server`, `crates/implant`, `crates/console`) — the
   "clean" C2 implementation from scratch, with forward secrecy, full task lifecycle,
   file transfer, SOCKS5 pivoting, and end-to-end tests.
2. **Legacy/Portal track** (`src/c2.rs`) — an earlier C2 endpoint router integrated with the
   portal's SQLite repository. This implements registration + polling but is superseded by the
   crates-based implementation.

The M2 milestone spec reads:

> Reuse portal auth/RBAC/audit. Multiple operators, roles (root/operator), full audit of tasking.
> `implant generate` command that parameterizes a profile (C2 endpoint, sleep, jitter, AES key)
> and emits a compiled binary. Jittered beacon scheduling. Task timeouts + cancellation.

---

## Capability map — M2 status

From the roadmap's Capability table (Section 3):

| Capability               | Status in codebase | Notes |
|--------------------------|--------------------|-------|
| Session lifecycle        | M1 (done)          | Beacon, sleep+jitter, reconnect — in `crates/implant/src/runtime.rs` |
| Tasking                  | M1 (done)          | Queue, timeout, interactive vs beacon, task outputs — `crates/server/src/queue.rs` |
| Transports               | M1 (done)          | HTTP/HTTPS C2 — `crates/server/src/channels.rs` + `crates/implant/src/transport.rs` |
| Operators                | **M2 (done)**      | Multiple operators, RBAC (admin/operator/viewer), audit trail — see below |
| Payloads                 | **M2 (done)**      | `implant generate` command + encrypted config blob — see below |

**Additionally, M3-M6 capabilities have been implemented ahead of schedule:**

| Capability               | Status in codebase | Notes |
|--------------------------|--------------------|-------|
| Stagers/stage-2          | Partially present  | No separate stager binary; config blob + runtime config baking covers parameterization |
| Other channels           | **M4 (done)**      | DNS channel — `crates/server/src/dns.rs` + `crates/profile/src/dns.rs` + `crates/implant/src/transport.rs` |
| TCP transport            | **Done**           | Raw length-framed TCP — `crates/server/src/tcp.rs` + `crates/implant/src/transport.rs` |
| Pivots                   | **M4 (done)**      | SOCKS5 proxy via `nw/socks` — `crates/implant/src/socks5.rs` + e2e test |
| Post-exploitation modules| **M4 (done)**      | `nw/download`, `nw/upload`, `nw/hashes`, `nw/sethost`, `nw/killtask`, `nw/exit` |
| Credentials              | Not started        | No credential store/harvest; deferred to M4 |
| Automation               | Not started        | No scripted task sequences/playbooks; deferred to M5 |
| Evasion                  | Not started        | No AMSI/ETW patch, sleep obfuscation, injection; deferred to M6 |
| Web operator console     | Not started        | CLI console only; web UI deferred to M7 |

---

## M2 items — detailed verification

### 1. Operator model (RBAC) — IMPLEMENTED

**Files:** `crates/server/src/operators.rs`

- `OperatorStore` with `authenticate`, `create`, `seed_default_admin`, `require`, `is_empty`
- `Role` enum: `Admin`, `Operator`, `Viewer` with `allows()` hierarchy enforcement
- `Role::allows()`: Admin > Operator > Viewer
- Argon2 password hashing + salt (via `argon2` crate)
- SQLite-backed (`c2_operators` table with FK to sessions)
- Tests: `seed_and_authenticate`, `create_operator_with_role`, `role_hierarchy`,
  `require_enforces_role_hierarchy`

**Console integration:** `crates/console/src/main.rs`
- `bootstrap_admin()` seeds initial admin from `NW_ADMIN_PASSWORD` or interactive prompt
- Operator login prompt (`login_prompt()`)
- Dispatcher carries the authenticated `Operator` identity

### 2. Audit trail — IMPLEMENTED

**Files:** `crates/server/src/audit_log.rs`

- `AuditLog` with `record()` and `list()`
- Every console command is audited: `action_name()` maps each command
  (`sessions`→`session.list`, `shell`→`task.shell`, `download`→`task.download`,
  `socks`→`task.socks`, `hashes`→`task.hashes`, `killjob`→`task.cancel`, etc.)
- Records: operator identity, target session, command details, success/failure, timestamp
- `c2_audit` table in SQLite
- Tests: `audit_records_and_queries`, `audit_entry_without_operator`
- E2E test asserts both successful and failed audit entries persist: `m2_auth_audit_and_restart_persistence`

### 3. Payload generation (`implant generate`) — IMPLEMENTED

**Console command:** `crates/console/src/generate.rs`

- `nw-console generate --endpoint <url> --psk <psk> --output <path> [--interval-ms N] [--jitter-ms N]`
- Invokes `cargo build --release -p nw-implant` with `NW_CFG`, `NW_INTERVAL`, `NW_JITTER` env vars
- Config blob is AES-GCM encrypted with XOR pepper masking (defeats static string search)

**Config encryption:** `crates/profile/src/config.rs`

- `encrypt_config(endpoint, psk)` → base64 blob: `[1B key_len][key XOR pepper][GCM ciphertext]`
- `decrypt_config(blob)` → recovers `(endpoint, psk)`
- Tests: `config_round_trips`, `different_builds_produce_different_blobs`,
  `garbage_blob_is_rejected`

**Implant config resolution:** `crates/implant/src/main.rs`

- `runtime_cfg()` decrypts `NW_CFG` at startup; falls back to `NW_ENDPOINT`/`NW_PSK` for dev
- Tests: `config_blob_obfuscates_callback`, `generated_binary_contains_no_plaintext_callback_or_psk`

**Legacy payload builder:** `src/payload.rs`

- `BuildRequest` struct with name, lhost, lport, psk, protocol, os, arch, interval_ms, jitter_ms, target
- `build()` function invokes `cargo build` with `--target` for cross-compilation
- Supports cross-compilation targets (rustup target add)
- `list()`, `download_path()`, `random_psk()`, `building_jobs()`, `recent_errors()`
- Tests: `sanitize_keeps_identifier_chars`, `build_id_and_binary_path`, `predict_file_matches_build_output_prefix`

### 4. Jittered beacon scheduling — IMPLEMENTED

**File:** `crates/implant/src/runtime.rs`

- `Profile` struct holds `interval` and `jitter` `Duration`s
- `sleep()` method: `interval + random(0..jitter_ms)`
- `next_id()` uses atomic counter for envelope IDs (nonce diversity)
- Configured via `NW_INTERVAL` and `NW_JITTER` env vars at build time

### 5. Task timeouts + cancellation — IMPLEMENTED

**Timeout:** `crates/implant/src/runner.rs`

- `run(Task)`: spawns command with `tokio::time::timeout(timeout_ms)`
- `kill_on_drop(true)` so aborting the task future kills the process
- Returns synthetic failure on timeout: "task timed out after {N}ms"
- Test: `command_is_terminated_at_its_task_timeout`

**Cancellation:** `crates/implant/src/runtime.rs`

- `nw/killtask` command: looks up the `AbortHandle` in `self.running` map and aborts
- `kill_task(task_id)` returns true if a child was terminated
- `spawn_tracked()` registers the handle; completion removes it
- E2E test: `killjob_cancels_an_in_flight_task`

**Task status tracking:** `crates/server/src/queue.rs`

- `TaskStatus` enum: `Pending`, `Delivered`, `Completed`
- `status()`, `statuses()`, `remove()` methods
- `c2_tasks` table with status column; `c2_task_results` table
- Console `jobs` command lists statuses; `killjob` cancels queued or in-flight tasks

### 6. Forward secrecy (M2 commit) — IMPLEMENTED

Commit `e68ee55 feat(c2): M2 forward secrecy + hashes console command`

**Files:** `crates/profile/src/crypto.rs`, `crates/server/src/channels.rs`,
`crates/implant/src/runtime.rs`

- x25519 key exchange at registration: both implant and server generate ephemeral keypairs
- `Register.session_key` carries the implant's ephemeral public key (base64)
- `RegisterAck.server_pub` carries the server's ephemeral public key
- `crypto::derive_key(shared_secret, SESSION_SALT)` derives the per-session AES-256 key
- Subsequent poll traffic uses the DH-derived session key, not the PSK
- `crypto::SESSION_SALT = b"nw-m2-session"`
- Tests: `key_exchange_produces_shared_secret_both_sides`, e2e register/poll round-trip

### 7. Hash computation (`nw/hashes`) — IMPLEMENTED

Commit `e68ee55`

**File:** `crates/implant/src/runtime.rs` (run_one, `nw/hashes` branch)

- SHA-256 hash of a file on the remote host
- Returns hex digest + path in stdout
- Test: `hashes_returns_sha256_of_a_remote_file` (e2e)

### 8. Sealed transport — IMPLEMENTED

From commit `25ed3e0 feat(c2): add Kelompok A features`

**Sealed envelope:** `crates/profile/src/envelope.rs`

- `Envelope::seal(key)` → outer transport frame: `{ session_id, blob=base64(nonce||ct) }`
- `Envelope::open(key, wire)` → decrypts the outer frame to recover the core envelope
- `Envelope::routing_id(wire)` → peeks the session_id without opening AEAD (for routing)
- Inner envelope is also AEAD-encrypted with per-envelope nonce (deterministic from id)
- Two-layer sealing: outer (random nonce, for transport) + inner (id-based nonce, for payload)

---

## Kelompok A features (commit `25ed3e0`) — IMPLEMENTED

| Feature | Status | File |
|---------|--------|------|
| Sealed transport | Done | `crates/profile/src/envelope.rs` |
| Task status | Done | `crates/server/src/queue.rs`, `crates/server/src/dispatch.rs` (`jobs` command) |
| File transfer (download) | Done | `crates/implant/src/download.rs` + `crates/server/src/filestore.rs` |
| File transfer (upload) | Done | `crates/implant/src/upload.rs` + `crates/server/src/uploadstore.rs` |
| SOCKS5 | Done | `crates/implant/src/socks5.rs` + `crates/server/src/channels.rs` (`nw/socks` dispatch) |

E2E tests: `download_streams_a_remote_file_to_the_server`,
`upload_streams_a_local_file_to_the_implant`, `socks5_proxy_relays_traffic_through_the_implant`

---

## Test coverage

### End-to-end (integration) tests — `crates/implant/tests/c2_end_to_end.rs`

| Test | What it verifies |
|------|----------------|
| `http_c2_round_trip` | HTTP C2: register → task `printf` → result returns |
| `download_streams_a_remote_file_to_the_server` | File download over HTTP C2 |
| `upload_streams_a_local_file_to_the_implant` | File upload over HTTP via Dispatcher |
| `socks5_proxy_relays_traffic_through_the_implant` | SOCKS5 proxy relays traffic through implant |
| `hashes_returns_sha256_of_a_remote_file` | `nw/hashes` task computes and returns SHA-256 |
| `redirect_delivers_its_result_to_the_second_listener` | `nw/sethost` redirect moves implant to new listener |
| `raw_tcp_c2_round_trip` | TCP transport C2: register → task → result |
| `raw_dns_c2_round_trip` | DNS transport C2: register → task → result |
| `killjob_cancels_an_in_flight_task` | `nw/killtask` aborts a running `sleep` |
| `m2_auth_audit_and_restart_persistence` | RBAC authz, audit trail, task timeout, SQLite persistence survives restart |

### Unit tests per crate

| Crate | File | Tests |
|-------|------|-------|
| `nw-profile` | `crypto.rs` | `derive_key_is_stable`, `key_exchange_produces_shared_secret_both_sides`, `encrypt_round_trips`, `decrypt_rejects_wrong_id`, `nonce_varies_by_id` |
| `nw-profile` | `envelope.rs` | `envelope_json_round_trip`, `seal_hides_frame_and_round_trips` |
| `nw-profile` | `config.rs` | `config_round_trips`, `different_builds_produce_different_blobs`, `garbage_blob_is_rejected` |
| `nw-profile` | `msgs.rs` | `messages_serialize` |
| `nw-profile` | `dns.rs` | `base32_round_trips`, `empty_round_trips`, `query_and_txt_response_round_trip`, `tx_split_across_long_strings`, `large_multi_label_qname_reply_parses` |
| `nw-server` | `dispatch.rs` | `sessions_lists_one`, `shell_requires_interaction`, `interact_then_shell_queues`, `redirect_queues_sethost_task`, `download_queues_nw_download_task`, `operator_can_queue_shell_tasks`, `viewer_cannot_queue_shell_tasks`, `every_operator_command_is_audited` |
| `nw-server` | `channels.rs` | `derive_is_stable`, `register_then_poll_round_trip` |
| `nw-server` | `session.rs` | `create_get_list_remove`, `sqlite_sessions_survive_store_restart` |
| `nw-server` | `queue.rs` | `push_drain_take_remove`, `remove_deletes_queued_task`, `status_tracks_pending_delivered_completed`, `sqlite_tasks_and_results_survive_store_restart` |
| `nw-server` | `operators.rs` | `seed_and_authenticate`, `create_operator_with_role`, `role_hierarchy`, `require_enforces_role_hierarchy` |
| `nw-server` | `audit_log.rs` | `audit_records_and_queries`, `audit_entry_without_operator` |
| `nw-server` | `uploadstore.rs` | `pushes_and_advances_cursor` |
| `nw-server` | `server.rs` | `configured_database_failure_is_not_silently_downgraded_to_memory` |
| `nw-server` | `config.rs` | `uses_local_defaults_when_environment_is_unset`, `uses_environment_values_when_present` |
| `nw-server` | `persist.rs` | `schema_initializes_clean` |
| `nw-implant` | `runtime.rs` | `jittered_sleep_stays_within_the_configured_window`, `completed_task_is_removed_from_cancellation_registry`, `sethost_rejects_relative_endpoint_without_mutating_runtime`, `sethost_rejects_non_http_scheme_without_mutating_runtime`, `sethost_switches_transport_cleanly` |
| `nw-implant` | `runner.rs` | `echo_captures_stdout`, `missing_binary_errors`, `command_is_terminated_at_its_task_timeout` |
| `nw-implant` | `transport.rs` | `scheme_selects_transport`, `tcp_addr_parsed`, `frame_has_length_prefix` |
| `nw-implant` | `download.rs` | `steps_and_resumes` |
| `nw-implant` | `upload.rs` | `writes_and_acks_contiguous` |
| `nw-implant` | `socks5.rs` | `relays_traffic_to_target_over_socks5` |
| `nw-console` | `main.rs` | `configured_password_bootstraps_initial_admin`, `weak_initial_admin_password_is_rejected` |
| `nw-console` | `generate.rs` | `config_blob_obfuscates_callback`, `generated_binary_contains_no_plaintext_callback_or_psk` |

---

## Not yet implemented (deferred to M4-M7)

**M4 — Post-exploitation modules & credential store:**
- No `crates/modules/` crate exists (the directory is empty)
- No credential harvesting/store (beyond session keys): no `creds` command, no LSASS dump,
  no saved password collection
- No process injection primitives (Windows hollowing, thread hijacking)

**M5 — Automation:**
- No scripted task sequences / operator playbooks
- No task graph with dependencies, retries, conditional gates
- No reuse of the `checks` catalog/runner as network task dispatch

**M6 — Evasion depth:**
- No AMSI/ETW patching
- No sleep obfuscation (encrypt in-memory region between beacons)
- No sandbox/VM checks
- No process hollowing or implant self-delivery

**M7 — Web operator console:**
- No web UI (the `naughtywolf` binary is the Sliver-style web GUI, but not the C2 console)
- No SSE/WS task stream over HTTP
- Console is CLI-only (`nw-console` REPL)

**Other transport channels (partially done):**
- DNS channel: implemented (server `serve_dns`, implant `dns_exchange`, shared codec in
  `crates/profile/src/dns.rs`) — e2e test passes
- SMB channel: **not implemented** (roadmap M4 says "dns, smb channels slit into transport trait")

---

## Legacy `src/c2.rs` (old-style C2 endpoints)

The root crate (`src/c2.rs`) has an older C2 router (`/c2/register`, `/c2/poll`) that uses
the portal's `Repository` (SQLite) instead of the crates-based `SessionRegistry`/`TaskQueue`.
It implements:

- Forward-secret x25519 registration + per-session key derivation (same crypto as the crates track)
- Registration records a callback in the `callbacks` table
- Poll endpoint: decrypts, validates session, updates last_seen, returns empty task list
  (no task dispatch — returns `Heartbeat` always)
- Test: `register_then_poll_records_callback_in_db`

This is effectively a **stripped-down M1/M2** that reuses the portal DB but does not dispatch
actual tasks. It appears to be a parallel/earlier implementation and is **not wired into the
main server binary** (no route registration in `src/main.rs`).
