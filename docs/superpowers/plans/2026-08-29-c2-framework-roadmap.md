# NaughtyWolf C2 — From-Zero Red Team Adversary Framework

Date: 2026-08-29
Status: approved architecture & roadmap
Scope: build a real, working C2 (implant + server + console) from zero in Rust

---

## 0. Honest framing

Cobalt Strike / Mythic / Sliver are multi-year, multi-engineer products. "Parity"
in one pass is fiction. What this roadmap builds is a **real, working adversary
framework** with the same architecture and capability categories, delivered in
verifiable vertical slices. Each milestone compiles, runs, and demonstrably
works before moving on. Feature breadth layers on top of a sound core.

Goal: a genuinely useful C2 that an operator can drive from a console against a
lab implant — capable of session management, tasking, pivoting, payload
generation, credential handling, and scripted automation.

---

## 1. Architecture

```
┌─────────────────────────────┐        ┌──────────────────────────────┐
│        OPERATOR             │        │          IMPLANT             │
│  console (cli + web ui)     │        │  runs on target host         │
└──────────────┬──────────────┘        └──────────────┬───────────────┘
               │  management/task api                  │  encrypted C2 channel
               ▼                                       ▼
┌─────────────────────────────┐        ┌──────────────────────────────┐
│       C2 SERVER             │        │   CHANNELS (transport)       │
│  listeners  │  task queue   │◄──────►│  http/https │ dns │ smb       │
│  sessions   │  graph        │  mTLS  └──────────────────────────────┘
│  payloads   │  creds        │   + per-channel obfuscation
│  pivots     │  automation   │
│  stagers    │  audit        │
└─────────────┴───────────────┘
        ▲
        │ sqlx / sqlite
┌───────┴─────────┐
│   PERSISTENCE   │  sessions, tasks, creds, loot, audit, graph
└─────────────────┘
```

### 1.1 Core separation
- **Implant** (`crates/implant`): standalone binary, no server deps. Owns its
  channel driver, command handlers, and in-process results. Reports back in
  structured envelopes.
- **Server** (`crates/server`): the authoritative brain. Listens, tracks
  sessions, dispatches tasks, stores everything. Never trusts the implant;
  every envelope is authenticated + validated.
- **Implant Profile** (`crates/profile`): shared structs for the wire envelope.
  This is the **protocol contract** both sides compile against.
- **Console** (`crates/console`): operator surface (REPL now, web UI later).

### 1.2 Language & reuse (locked)
- All Rust, 2024 edition, matches existing axum/tokio/sqlx toolchain.
- Reuse current crates: `sqlx` (persistence), `tokio` (async), `axum` +
  `tower-sessions` + `tower-sessions-sqlx-store` (web/console + auth), `clap`
  (CLI), `tracing` (logs).
- New deps, chosen lazily only when the stdlib / current deps fall short:
  - `rand` (nonce, keygen) — not present yet
  - `aes-gcm` + `chacha20poly1305` (channel crypto via RustCrypto AEAD)
  - `x25519-dalek` (key exchange if we do forward secrecy)
  - `base64` (envelope encoding over text transports)
  - eventual: `dns` (raw UDP for DNS channel) — defer until milestone 5

### 1.3 Workspace layout (new `crates/` parent)
```
crates/
  profile/     # wire envelopes + crypto primitives (shared, no server logic)
  implant/     # implant binary
  server/      # C2 server library + binary
  console/     # operator console
  modules/     # post-exploitation module interface + registry
legacy/        # (optional) move old portal code here, out of the build
```

---

## 2. Protocol contract (the heart)

Single `Envelope` both sides speak. Versioned, authenticated, length-framed.

```rust
// crates/profile/src/envelope.rs
pub struct Envelope {
    pub version: u8,          // protocol version
    pub id: u64,              // unique envelope id
    pub session_id: Option<Uuid>,
    pub kind: Kind,
    pub encrypted: Vec<u8>,   // AEAD ciphertext of the inner payload
    pub nonce: [u8; 12],
}

pub enum Kind { Register, Task, Result, Beacon, Heartbeat, Kill, ... }

// Encrypted inner
pub struct Register { pub hostname, pub pid, pub arch, pub os, pub username, pub addr }
pub struct Task { pub id: Uuid, pub command: String, pub args: Vec<String>, pub timeout_ms: u64 }
pub struct TaskResult { pub id: Uuid, pub ok: bool, pub stdout: Vec<u8>, pub stderr: Vec<u8>, pub exit_code: i32 }
```

- **Auth**: each implant gets a per-session symmetric key (server holds a key
  ring). Real C2s use mTLS + per-session keys; we start with x25519 key
  exchange at registration, then AEAD per session.
- **Framing**: length-prefixed binary over TCP/WebSocket; base64 + custom
  envelope over HTTP body; per-byte for DNS.

---

## 3. Capability map (what "like Cobalt Strike" breaks into)

| Capability | C2 feature | Status today |
|---|---|---|
| Session lifecycle | beacon/session model, sleep+JITTER, reconnect | M1 |
| Tasking | queue, timeout, interactive vs beacon, task outputs | M1 |
| Transports | http/https C2 | M1 |
| Operators | multiple operators, roles, audit trail | M2 (reuse portal auth) |
| Payloads | generate/compile implant, obfuscate | M2 |
| Stagers | small first-stage that loads stage-2 | M3 |
| Other channels | dns, smb added to profile | M4 |
| Pivots | jump hosts, socks, reverser | M4 |
| Post-exploitation | modules (enum, creds, lateral, etc.) | M4 |
| Credentials | harvest & store creds, reuse | M4 |
| Automation | scripted tasking / operator plugins | M5 |
| Evasion | AMSI/ETW patch, sleep obfuscation, injection | M6 |
| UI | web operator console | M7 |

---

## 4. Milestones (each fully compiles + runs + is testable)

### M1 — HTTP C2 vertical slice (foundation)
**Do**: crates/profile (envelope + crypto + key exchange), crates/implant
(basic http/https beacon, register, poll tasks, run simple commands, exfil
results), crates/server (https listener, session store, task queue, dispatch),
crates/console (REPL: sessions, interact, run `shell ...`).
**Done when**: compile one implant, register it against the server, task it
from the console, see output return. Integration test drives the loop.
**Proof**: `tests/c2_end_to_end.rs` spins server + in-process implant, asserts a
task round-trip.

### M2 — Operator model + payload generation
Reuse portal `auth`/RBAC/audit. Multiple operators, roles (root/operator),
full audit of tasking. `implant generate` command that parameterizes a profile
(C2 endpoint, sleep, jitter, AES key) and emits a compiled binary. Jittered
beacon scheduling. Task timeouts + cancellation.

### M3 — Stagers + stage-2
Small stager binary downloads + executes stage-2 in-memory. Process injection
primitives (platform-gated, Windows-first via `windows-sys`). Sleep
obfuscation (encrypt in-memory region between beacons).

### M4 — More transports + pivoting + modules
dns + smb channels slit into transport trait. Socks proxy via implant.
Module interface (`crates/modules`) + registry; first useful modules
(host enum, credential cache read, download/upload). Credential store (reuse
portal creds concepts).

### M5 — Automation
Script-specified task sequences / operator playbooks (text format, not a new
language — reuse existing portal report/planning data model). Task graph:
dependencies, retries, conditional gates. Reuse `checks` catalog/runner ideas
from current codebase but as network task dispatch.

### M6 — Evasion depth
AMSI/ETW patch, process hollowing (windows-sys), sandbox/VM checks, sleep
obfuscation harden, implant self-delivery. Document that this is
defense-research / authorized-lab-only.

### M7 — Web operator console
Reuse axum portal + tower-sessions UI: live session list, task stream (SSE/WS,
reuse `sse.rs`/`ws.rs` patterns), payload management. Full console parity with
the CLI.

---

## 5. Cross-cutting requirements (every milestone)

- **Never trust the implant**: validate every envelope, cap sizes, require auth.
- **Audit everything**: every operator action logged (reuse `audit` module).
- **Isolation**: module code gated by platform cfg; lab-only guardrails.
- **Tests at each milestone**: round-trip + unit tests in `tests/`. The
  end-to-end loop test is the contract that must keep passing.
- **Authz boundaries baked in from M1**: implant can only act within its
  session scope; operators scoped by role.

---

## 6. Ethics / legal guardrail (non-negotiable)

This is an **authorized security-lab framework**. It builds offensive
primitives (injection, evasion, covert C2) that have real abuse potential.
Apply to and test ONLY against assets you own or are explicitly authorized to
test. Production use requires an HTTPS/mTLS transport, strong secrets, and
defensive isolation. Any feature is gated to lab/authorized contexts.

---

## 7. Start now

Milestone 1. See `docs/superpowers/plans/2026-08-29-c2-m1-http-slice.md` for
the concrete M1 task breakdown (next doc to author before any code).
