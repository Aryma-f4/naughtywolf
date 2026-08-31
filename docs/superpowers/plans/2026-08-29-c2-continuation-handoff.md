# NaughtyWolf C2 — Continuation Handoff Plan (for next AI agent)

Date: 2026-08-29
Format: handoff plan to continue development. Read this + the two M1 docs first.
Audience: a fresh AI agent (or human) picking up where the session stopped.

---

## 0. TL;DR — where things stand

- We are building a **real C2 adversary framework from zero, in Rust** under
  `crates/`, per the approved roadmap:
  `docs/superpowers/plans/2026-08-29-c2-framework-roadmap.md`
- **M1 (HTTP C2 vertical slice) plus callback/deploy polish is complete.**
  Implant registers, phones home, pulls tasks, runs them, exfils results; server
  + console drive it. Implants can now receive `nw/sethost` through the console
  `redirect <host>` command.
- `ServerConfig` reads `NW_BIND`, `NW_PSK`, and `NW_CALLBACK_HOST`; C2 Docker
  artifacts are `Dockerfile.c2`, `docker-compose.c2.yml`, and `.env.example.c2`.

---

## 1. Architecture (one screen)

```
crates/                       # NEW workspace members (real C2)
  profile/  nw-profile        # wire protocol: Envelope, Kind, msgs, crypto (AES-256-GCM) — shared contract
  server/   nw-server         # C2 brain: HTTP listener, SessionRegistry, TaskQueue, Dispatcher
  implant/  nw-implant        # beacon: registers, polls, runs tasks, exfils; BeaconRuntime + runner
  console/  nw-console        # operator REPL (bins: nw-server, nw-implant, nw-console)
```

Workspace root = existing `naughtywolf` package. Root `Cargo.toml` =
`[workspace]` + the legacy package deps. Members listed in `[workspace].members`.

### Module map — server (`crates/server/src/`)
- `lib.rs` — re-exports `Dispatcher, Outcome, ServerState, serve`.
- `server.rs` — `ServerState { registry, queue, psk }` + `serve(state, bind)` binds axum + runs.
- `channels.rs` — HTTP router: `POST /c2/register`, `POST /c2/poll`. **Crypto AAD rule lives here (important, see §3).**
- `session.rs` — `Session` + `SessionRegistry` (in-memory HashMap behind Mutex).
- `queue.rs` — per-session `TaskQueue` (pending tasks + delivered results).
- `dispatch.rs` — operator command layer (`sessions`, `interact`, `shell`, `kill`), holds current interacted session.

### Module map — implant (`crates/implant/src/`)
- `runtime.rs` — `Profile`, `discover_profile()`, `BeaconRuntime` (loop: register→poll→execute→sleep), `run()`.
- `runner.rs` — `run(task)` executes a process with timeout, returns `TaskResult`; `run_blocking` for tests.
- `main.rs` — reads `NW_ENDPOINT`, `NW_PSK`, `NW_INTERVAL`, `NW_JITTER`.

### Module map — profile (`crates/profile/src/`)
- `envelope.rs` — `Envelope` (version, id, session_id, kind, encrypted), `Kind`.
- `msgs.rs` — `Register`, `RegisterAck`, `Task`, `TaskResult`, `PollRequest`.
- `crypto.rs` — x25519 KeyPair (unused in M1), `derive_key` (HKDF-SHA256), `encrypt`/`decrypt` (AES-256-GCM), `nonce_for`.

---

## 2. How the round trip works (the contract every change must keep green)

1. Implant `register()` → `Envelope(Kind::Register, id=N, encrypted=AEAD(Register_json))` → `POST /c2/register`.
2. Server decrypts with **session key**, creates a `Session`, replies `Envelope(Kind::RegisterAck, id=N+1, AEAD(RegisterAck{session_id}))`.
3. Implant stores session_id; loop: `poll(sid)` sends `PollRequest{results}` → server drains pending tasks → replies encrypted `Vec<Task>`.
4. Implant `execute()`s tasks, buffers `TaskResult`s, delivers them on the next poll.
5. Console `Dispatcher` queues tasks; console polls `queue.take_result()` for output.

**Session key model (M1):** both sides derive it from a **shared PSK**:
`key = crypto::derive_key(psk, b"nw-m1-salt")`. Implant sends its psk base64 in
`Register.session_key` but the **server ignores that field** for derivation and
uses its own `state.psk` — keep both sides' PSK identical, else AEAD fails.

---

## 3. CRITICAL invariants (do not break)

- **AAD == reply envelope id.** The server encrypts every reply with AAD equal
  to the **reply** `Envelope.id`, which is `request.id + 1`. The implant decrypts
  with `reply.id`. If you change id/encryption logic, keep
  `ciphertext_AAD == the Envelope`'s own `id` you ship. This bug already bit us
  once; the e2e test catches it.
- **Do not hold a std RwLock/Mutex guard across `.await`.** `BeaconRuntime::run`
  copies `session_id` out before awaiting (see the fixed version). Any new lock
  used in async must be copied to a local then released before `.await`, else
  tokio::spawn fails `Send` + deadlocks.
- **M1 uses PSK**; the x25519 `KeyPair` in crypto.rs is scaffolding for M2
  forward secrecy — don't wire it into the PSK path yet.
- **Test contract:** `crates/implant/tests/c2_end_to_end.rs` is the DO NOT
  REGRESS test. If the round trip breaks, fix the root cause, don't monkey-patch.

---

## 4. Completed so far (this session)

- [x] Workspace scaffolding + 4 crates.
- [x] `nw-profile`: envelope, msgs, AES-256-GCM crypto + HKDF, unit tests.
- [x] `nw-server`: listener (register/poll), SessionRegistry, TaskQueue,
      Dispatcher, `ServerConfig`, and unit tests.
- [x] `nw-implant`: BeaconRuntime loop, runner with timeout, `nw/exit` implicit stop.
- [x] `nw-console`: REPL (sessions/interact/shell/redirect/kill/exit/help) + result wait.
- [x] **e2e round trip test passing** (server + in-process implant over real HTTP).
- [x] `cargo test --workspace` green, no warnings.
- [x] Roadmap + M1 plan docs written.
- [x] Dynamic callback: `nw/sethost <host>` changes the implant endpoint; console
      `redirect <host>` queues it for the focused session.
- [x] Deploy configuration: `crates/server/src/config.rs` reads `NW_BIND`,
      `NW_PSK`, and `NW_CALLBACK_HOST` with local defaults.
- [x] C2 container artifacts: `Dockerfile.c2`, `docker-compose.c2.yml`, and
      `.env.example.c2`; `README.md` documents native and Compose workflows.
- [x] Callback redirect e2e coverage: `crates/implant/tests/c2_end_to_end.rs`.

## 5. Completed continuation: easy deploy + dynamic callback host

This continuation is complete. Preserve the C2/portal boundary and the M1
transport invariants in subsequent work.

### 5.1 Dynamic callback host (implant) — complete

`BeaconRuntime` keeps its endpoint mutable, `nw/sethost <base-url>` updates it
and returns an acknowledgement, and the console's `redirect <host>` queues that
implant-local command. `crates/implant/tests/c2_end_to_end.rs` verifies that a
redirect result reaches the second listener.

### 5.2 Easy deploy (server) — complete

`nw-server` loads `ServerConfig` from `NW_BIND`, `NW_PSK`, and
`NW_CALLBACK_HOST` (defaulting to `127.0.0.1:8081`, `dev-psk-change-me`, and
`http://127.0.0.1:8081`). `NW_CALLBACK_HOST` is reserved and inert in M1: it is
loaded and exposed to retain the continuation plan's single environment surface,
but the server does not consume, advertise, log, or distribute it. Operators
explicitly set an implant's `NW_ENDPOINT` or issue `redirect <host>` to an
existing implant. Easy-deploy completion covers the `ServerConfig` environment
surface and C2 Docker/Compose artifacts, not automatic callback propagation.

### 5.3 Deployability polish — complete

The README has a concise C2 quickstart. Configuration remains environment-only;
there is no `c2.toml` surface.

---

## 6. Roadmap — later milestones (from the roadmap doc)

| M | Deliverable | Notes |
|---|---|---|
| M2 | Operator model + payload generation | Reuse portal `auth`/RBAC/audit; `implant generate`; jittered beacon; task timeouts/cancel |
| M3 | Stagers + stage-2 | small stager loads stage-2 in memory; process injection (windows-sys); sleep obfuscation |
| M4 | dns/smb channels + pivoting + modules | transport trait; socks; `crates/modules` registry + first modules; credential store |
| M5 | Automation | scripted task sequences, task graph (deps/retries/gates) |
| M6 | Evasion depth | AMSI/ETW patch, hollowing, sandbox checks |
| M7 | Web operator console | reuse portal axum UI, session list + task stream (SSE/WS) |

Full design + build order: `docs/superpowers/plans/2026-08-29-c2-framework-roadmap.md`.
M1 details: `docs/superpowers/plans/2026-08-29-c2-m1-http-slice.md`.

---


## 7. First concrete steps for the next agent
1. Re-verify baseline: `cargo test --workspace` green.
2. Read `docs/superpowers/plans/2026-08-29-c2-framework-roadmap.md` + M1 doc.
3. [x] Implement §5.1 (dynamic host) — mutable endpoint + `nw/sethost` + `redirect`.
4. [x] Implement §5.2 (deploy) — `ServerConfig` + `Dockerfile.c2` + `docker-compose.c2.yml` + README.
5. [x] `cargo test --workspace` green, then update this doc's §4/§5 checklists.
