# M1 — HTTP C2 Vertical Slice (task breakdown)

Parent: `docs/superpowers/plans/2026-08-29-c2-framework-roadmap.md`
Status: ready to build

## Goal
A working end-to-end HTTP(S) C2: implant registers, phones home, pulls tasks,
runs commands, returns output; operator drives it from a console. Nothing after
M1 matters if this round-trip is not real and tested.

## Workspace scaffold
- Convert to a cargo workspace with members: `crates/profile`, `crates/server`,
  `crates/implant`, `crates/console`. Move current portal into `legacy/` kept
  out of the workspace root build (defer to M2 to avoid churn this milestone —
  M1 only adds `crates/` alongside existing code).

## Deliverables

### 1. `crates/profile` — protocol contract (no server logic)
- `envelope.rs`: `Envelope`, `Kind`, version const, `Encoded` length-prefix + base64.
- `crypto.rs`: x25519 key exchange provider (client ECDH -> shared secret),
  derive per-session AES-256-GCM key via HKDF, `encrypt`/`decrypt`, key ring trait.
- `register.rs` / `task.rs` / `task_result.rs`: message structs (see parent doc §2).
- Tests: round-trip encrypt/decrypt, envelope encode/decode, key-derivation stable.

### 2. `crates/server` — C2 brain
- `listener.rs`: axum app with two routes:
  - `POST /c2/register` -> returns session id + server ECDH pubkey
  - `POST /c2/poll` -> implant sends encrypted (task_ids_done + new key req),
    server returns encrypted pending tasks
- `session.rs`: in-memory + sqlite-backed session registry (reuse sqlx pool).
- `queue.rs`: task queue per session, dispatch rules, timeout.
- `dispatch.rs`: built-in command executor on server side for M1 (server-side
  jobs like `sessions`, `kill`, `taskinfo`, and `shell` which forwards to the
  implant for remote exec). Distinguish local console commands vs remote tasks.
- Persistence: sessions, tasks, results into sqlite (reuse migrations dir).

### 3. `crates/implant` — beacon
- `main.rs`: parse profile (endpoint, interval, jitter, aes key), loop:
  register once, then every `interval +/- jitter`, POST `/c2/poll` with a
  session key, process returned `Task`s, run supported commands (`shell`,
  `hostname`, `whoami`, `exit`), POST results.
- `runner.rs`: command execution (std::process), timeout handling, capture
  stdout/stderr/exit code.
- Resistant to basic errors: network retries, backoff.

### 4. `crates/console` — operator REPL
- `main.rs`: clap + interactive loop.
- Commands: `sessions` (list), `interact <id>`, `tasks <id>`, `kill <id>`,
  `shell <cmd>` within an interacted session, `help`, `exit`.
- Talks to server via the same gRPC-free local API (library calls, not network,
  in M1; the console links `crates/server` as a lib).

## Definition of done (all must pass)
1. `cargo build --workspace` clean.
2. `tests/c2_end_to_end.rs`: spin server in-process, register an in-process
   implant, dispatch `shell echo hello`, assert result arrives with `hello`.
3. `cargo test --workspace` green (profile unit + e2e).
4. Manual: run server, run compiled implant binary with a profile, drive REPL,
   task it, see output. (Authorized lab/localhost only.)

## Non-goals this milestone
- No stagers, no DNS/SMB, no pivoting, no modules, no UI, no automation, no
  OPSEC/evasion. Those are M3-M7.

## Build order
1. workspace scaffold + profile crate (unit tests green)
2. server listener/session/queue/dispatch (tests green)
3. implant beacon + runner (tests green)
4. console REPL
5. e2e test + manual run
