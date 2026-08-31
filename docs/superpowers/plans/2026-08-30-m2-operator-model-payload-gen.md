# M2 — Operator Model + Payload Generation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete M2 milestone by adding operator authentication/RBAC/audit to the nw-server console, an `implant generate` payload generation command, and SQLite persistence for sessions and tasks.

**Architecture:** The `crates/server` (nw-server) and `crates/console` (nw-console) crates gain a lightweight operator model that mirrors the portal's existing auth/RBAC/audit patterns but runs in-process (no web UI). `implant generate` lives in the console crate, builds an encrypted config blob via `nw_profile::config::encrypt_config`, and invokes `cargo build` with `--cfg` flags. Session/task persistence moves from in-memory maps to SQLite via sqlx.

**Tech Stack:** Rust 2024, sqlx (sqlite), argon2 (password hashing), clap (CLI), tower-sessions (optional, future web console), tracing (audit logs). Reuse existing code: `src/auth/{password,rbac}.rs`, `src/audit.rs`, `crates/profile/src/config.rs`.

**Spec:** `docs/superpowers/plans/2026-08-29-c2-framework-roadmap.md#m2`

## Global Constraints

- Edition 2024; all workspace members share `[workspace.dependencies]` (no version drift)
- `crates/server` may depend on `sqlx`, `argon2`, `clap`; `crates/console` may depend on `nw-server` + same set
- `crates/profile` stays transport-only (no server logic or auth deps)
- All existing e2e tests must continue passing (`crates/implant/tests/c2_end_to_end.rs`)
- Authorized lab/localhost only — document this
- Commit after each task; `cargo test --workspace` must stay green

---

## File Structure

### New files

- `crates/server/src/operators.rs` — in-process operator store (password auth, roles)
- `crates/server/src/audit_log.rs` — append-only audit trail for operator actions
- `crates/server/src/persist.rs` — SQLite-backed SessionRegistry + TaskQueue implementations
- `crates/console/src/generate.rs` — `implant generate` command implementation
- `crates/server/Cargo.toml` (modify) — add sqlx, argon2, clap deps
- `crates/console/Cargo.toml` (modify) — add clap derive deps
- `migrations/003_c2_sessions.sql` — session + operator tables for nw-server SQLite

### Modified files

- `crates/server/src/session.rs` — add `SqliteSessionRegistry` (swap in persist.rs)
- `crates/server/src/queue.rs` — add `SqliteTaskQueue` (swap in persist.rs)
- `crates/server/src/server.rs` — wire persistence layer into `ServerState`
- `crates/server/src/lib.rs` — re-export new types
- `crates/server/src/config.rs` — add `NW_OPERATORS_DB` or reuse existing db path
- `crates/console/src/main.rs` — add login prompt + `generate` subcommand
- `crates/console/Cargo.toml` — add `derive` feature for clap
- `crates/implant/tests/c2_end_to_end.rs` (modify) — add auth token to test harness

---

## Task 1: SQLite-backed session + task persistence

**Files:**
- Create: `migrations/003_c2_sessions.sql`
- Create: `crates/server/src/persist.rs`
- Modify: `crates/server/src/session.rs` (add SqliteSessionRegistry)
- Modify: `crates/server/src/queue.rs` (add SqliteTaskQueue)
- Modify: `crates/server/src/server.rs` (wire persistence into ServerState)
- Modify: `crates/server/Cargo.toml` (add sqlx with sqlite feature)
- Modify: `crates/server/src/config.rs` (add `NW_DB` config)

**Interfaces:**
- Consumes: `ServerConfig` from `config.rs`, `Session`/`SessionRegistry` from `session.rs`
- Produces: `SqliteSessionRegistry`, `SqliteTaskQueue` implementing same `pub` API surface as in-memory versions

**Goal:** Sessions and task results survive server restarts via SQLite. The e2e tests use in-memory SQLite (`:memory:`) so they keep working without an on-disk file.

- [x] **Step 1: Write the migration**

```sql
-- Session storage for nw-server C2. Reuses the portal's sqlite db or a
-- dedicated NW_DB. Schema mirrors the in-memory Session struct.
CREATE TABLE IF NOT EXISTS c2_sessions (
    id TEXT PRIMARY KEY,
    hostname TEXT NOT NULL,
    username TEXT NOT NULL,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    pid INTEGER NOT NULL,
    addr TEXT NOT NULL,
    session_key BLOB NOT NULL,  -- 32-byte AES key
    last_seen TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS c2_tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
    command TEXT NOT NULL,
    args_json TEXT NOT NULL,  -- JSON-encoded Vec<String>
    timeout_ms INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'delivered', 'completed')) DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS c2_task_results (
    task_id TEXT PRIMARY KEY REFERENCES c2_tasks(id) ON DELETE CASCADE,
    ok INTEGER NOT NULL,
    stdout BLOB NOT NULL,
    stderr BLOB NOT NULL,
    exit_code INTEGER NOT NULL,
    completed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

- [x] **Step 2: Add sqlx to nw-server Cargo.toml**

```toml
sqlx = { workspace = true, features = ["runtime-tokio-rustls", "sqlite"] }
```

- [x] **Step 3: Write the persistence layer**

`crates/server/src/persist.rs`:

```rust
use sqlx::{Pool, Sqlite, SqlitePool};
use std::sync::Arc;
use uuid::Uuid;

use crate::queue::{TaskQueue, QueueError, SharedQueue};
use crate::session::{Session, SessionRegistry, SharedRegistry};
use nw_profile::msgs::{Task, TaskResult};
use nw_profile::crypto;

/// SQLite-backed SessionRegistry. Implements the same interface as the
/// in-memory SessionRegistry so callers are unaffected.
pub struct SqliteSessionRegistry {
    pool: SqlitePool,
}

impl SqliteSessionRegistry {
    pub async fn new(pool: SqlitePool) -> Self {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS c2_sessions (
                id TEXT PRIMARY KEY, hostname TEXT NOT NULL, username TEXT NOT NULL,
                os TEXT NOT NULL, arch TEXT NOT NULL, pid INTEGER NOT NULL,
                addr TEXT NOT NULL, session_key BLOB NOT NULL,
                last_seen TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            )"
        ).execute(&pool).await.ok();
        SqliteSessionRegistry { pool }
    }

    pub fn pool(&self) -> &SqlitePool { &self.pool }
}

impl SessionRegistry for SqliteSessionRegistry {
    fn create(&self, hostname: String, username: String, os: String, arch: String,
              pid: u32, addr: String, key: [u8; crypto::KEY_LEN]) -> Uuid {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now().to_rfc3339();
        // fire-and-forget; persistence best-effort
        let _ = futures::executor::block_on(sqlx::query(
            "INSERT INTO c2_sessions (id, hostname, username, os, arch, pid, addr, session_key, last_seen) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
        ).bind(id.to_string()).bind(hostname).bind(username).bind(os).bind(arch)
         .bind(pid as i64).bind(addr).bind(&key[..]).bind(&now).execute(&self.pool));
        id
    }
    // ... get, list, touch, remove implementations
}
```

> **Note:** The `SessionRegistry` trait doesn't exist yet — it's a struct. This task introduces a trait abstraction so we can swap in-memory vs SQLite. See `session.rs` for current struct impl.

- [x] **Step 4: Add tests for round-trip persistence**
- [x] **Step 5: Wire into ServerState**
- [ ] **Step 6: Commit**

---

## Task 2: Operator authentication (in-process, no web)

**Files:**
- Create: `crates/server/src/operators.rs`
- Modify: `crates/console/src/main.rs` (login prompt)
- Modify: `crates/server/Cargo.toml` (add argon2)
- Modify: `crates/server/src/lib.rs` (re-export)

**Interfaces:**
- Consumes: `Role` from portal's `src/auth/rbac.rs` (copied/adapted into nw-server), `hash_password`/`verify_password` from `src/auth/password.rs`
- Produces: `Operator` struct, `OperatorStore` with `authenticate()`, `require()` for role checks

**Goal:** Console prompts for username/password at startup. Operators have roles: admin (full), operator (session management), viewer (read-only). Passwords hashed with argon2.

- [x] **Step 1: Port RBAC + password hashing into nw-server**

```rust
// crates/server/src/operators.rs
pub use nw_profile::rbac::Role;  // re-export from profile or copy

pub struct Operator {
    pub id: Uuid,
    pub username: String,
    pub role: Role,
}

pub struct OperatorStore {
    pool: SqlitePool,
}

impl OperatorStore {
    pub async fn create_pool(db_path: &str) -> SqlitePool { ... }
    
    pub async fn authenticate(&self, username: &str, password: &str) -> Option<Operator> {
        // Look up user, verify argon2 hash, check disabled flag
    }
    
    pub fn require(&self, operator: &Operator, required: Role) -> Result<(), String> {
        if operator.role.allows(required) { Ok(()) } 
        else { Err("insufficient privileges".into()) }
    }
}
```

- [x] **Step 2: Add operator table to migration**

```sql
CREATE TABLE IF NOT EXISTS c2_operators (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'viewer')),
    disabled INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

- [x] **Step 3: Seed default admin user on first startup**
- [x] **Step 4: Write tests for auth + RBAC**
- [ ] **Step 5: Commit**

---

## Task 3: Audit trail for operator actions

**Files:**
- Create: `crates/server/src/audit_log.rs`
- Modify: `crates/server/src/dispatch.rs` (wrap parse() with audit)
- Modify: `crates/server/Cargo.toml`

**Interfaces:**
- Consumes: `Operator` from Task 2
- Produces: `AuditLog::record(operator_id, action, session_id, details)`

**Goal:** Every console command (`shell`, `kill`, `upload`, `redirect`, `socks`, etc.) is logged with who did it, when, and what session/target. Stored in SQLite alongside `c2_sessions`.

```sql
CREATE TABLE IF NOT EXISTS c2_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    operator_id TEXT,
    action TEXT NOT NULL,
    target_session TEXT,
    details TEXT,
    succeeded INTEGER NOT NULL,
    timestamp TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

- [x] **Step 1: Write audit_log.rs with AuditLog struct**
- [x] **Step 2: Wrap Dispatcher::parse() to record actions**
- [x] **Step 3: Write tests**
- [ ] **Step 4: Commit**

---

## Task 4: `implant generate` command

**Files:**
- Create: `crates/console/src/generate.rs`
- Modify: `crates/console/src/main.rs` (add clap subcommand)
- Modify: `crates/console/Cargo.toml` (add clap derive)
- Modify: `crates/implant/Cargo.toml` (ensure nw-profile is a dep for config)

**Interfaces:**
- Consumes: `nw_profile::config::encrypt_config`, `nw_profile::crypto`
- Produces: `generate_implant()` that bakes `NW_CFG` and invokes `cargo build`

**Goal:** `nw-console generate --endpoint http://c2:8081 --psk <psk> --output /tmp/implant.bin` produces a standalone binary with the callback baked in as an encrypted config blob (`NW_CFG`), not a plaintext string.

The command flow:
1. Call `nw_profile::config::encrypt_config(endpoint, psk)` → base64 blob
2. Invoke `cargo build --release` with `NW_CFG=<blob>` set as a build-time env var
3. Copy the resulting binary to `--output` path

```rust
// crates/console/src/generate.rs
pub async fn generate_implant(
    endpoint: &str,
    psk: &str,
    output: &Path,
    interval_ms: Option<u64>,
    jitter_ms: Option<u64>,
) -> anyhow::Result<()> {
    let blob = nw_profile::config::encrypt_config(endpoint, psk)?;
    // Run cargo build with the cfg injected
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(&["build", "--release", "-p", "nw-implant"])
        .env("NW_CFG", &blob)
        .env("NW_INTERVAL", interval_ms.unwrap_or(5000).to_string())
        .env("NW_JITTER", jitter_ms.unwrap_or(1000).to_string());
    // ... build, then copy target/release/nw-implant to output
}
```

- [x] **Step 1: Write the generate module with tests for config encryption**
- [x] **Step 2: Add clap subcommand to console**
- [x] **Step 3: Test: generate a binary, verify it contains encrypted config not plaintext**
- [ ] **Step 4: Commit**

---

## Task 5: Wire auth + persistence into console

**Files:**
- Modify: `crates/console/src/main.rs` (login flow, pass operator to Dispatcher)
- Modify: `crates/server/src/dispatch.rs` (operator-aware parse)

**Interfaces:**
- Consumes: `OperatorStore` from Task 2, `SqliteTaskQueue`/`SqliteSessionRegistry` from Task 1
- Produces: Console that requires login before accepting commands; all actions audited

**Goal:** Console prompts for credentials, loads persistence layer from SQLite, all tasking is role-checked and audited.

- [x] **Step 1: Add login prompt to console REPL**
- [x] **Step 2: Pass authenticated Operator into Dispatcher**
- [x] **Step 3: Add `role` field to `Outcome::TaskQueued` for audit filtering**
- [x] **Step 4: Test: viewer cannot run `shell`, operator can**
- [ ] **Step 5: Commit**

---

## Task 6: E2E integration test for auth + audit

**Files:**
- Modify: `crates/implant/tests/c2_end_to_end.rs` (add auth-aware harness)
- Modify: `crates/server/src/audit_log.rs` (expose query API for tests)

**Interfaces:**
- Consumes: `SqliteSessionRegistry`, `SqliteTaskQueue`, `OperatorStore`, `AuditLog`
- Produces: Tests proving auth + audit work end-to-end

**Goal:** Integration test that spawns server with SQLite persistence + operator db, authenticates as admin, tasks an implant, verifies audit log shows the action, shuts down, restarts server, sessions/tasks persist.

- [x] **Step 1: Write the integration test**
- [x] **Step 2: Run and fix until green**
- [ ] **Step 3: Commit**

---

## Execution Order

```
Task 1 → Task 2 → Task 3 → Task 5 → Task 4 → Task 6
    (1 needs 2 for operator db; 2 needs 1 for sqlite; 3 needs 1+2;
     5 needs 1+2+3; 4 is independent; 6 needs all
```

Task 4 (generate) is independent of the auth/persistence line and can go in parallel.
```
