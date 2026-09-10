# GSocket Transport Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generate GSocket-capable implants that register and operate as callbacks in the NaughtyWolf web portal.

**Architecture:** GSocket provides a transparent TCP tunnel around NaughtyWolf's sealed length-framed protocol. The portal owns callback state and exposes one shared repository-backed processor to HTTP and raw TCP listeners; the implant manages a local `gs-netcat` forwarding child.

**Tech Stack:** Rust 2024, Tokio, Axum, SQLx/SQLite, Docker Compose, `gs-netcat`

**Spec:** `docs/superpowers/specs/2026-09-09-gsocket-transport-design.md`

## Global Constraints

- Preserve HTTP, HTTPS, raw TCP, DNS, and SMB behavior.
- Never invoke a shell or download/install GSocket at runtime.
- Keep `NW_PSK` and `GS_SECRET` separate and redacted.
- Raw TCP binds privately by default and rejects frames larger than 8 MiB.
- Existing encrypted payload profiles and metadata remain readable.

---

### Task 1: Versioned payload transport configuration

**Files:**
- Modify: `crates/profile/src/config.rs`
- Modify: `src/payload.rs`
- Test: `crates/profile/src/config.rs`
- Test: `src/payload.rs`

**Interfaces:**
- Produces: `PayloadConfig { endpoint, psk, gsocket: Option<GSocketConfig> }`
- Produces: `encrypt_payload_config(&PayloadConfig)` and `decrypt_payload_config(&str)` with legacy compatibility

- [ ] Write failing tests proving a GSocket secret round-trips, legacy blobs still decode, and metadata serialization omits the secret.
- [ ] Run `cargo test -p nw-profile config -- --nocapture` and confirm failure because the new types/functions do not exist.
- [ ] Add the versioned serde structures and compatibility wrappers; extend `BuildRequest` with an optional GSocket secret and local port.
- [ ] Run the focused tests and confirm they pass.

### Task 2: Managed GSocket implant transport

**Files:**
- Modify: `crates/implant/src/transport.rs`
- Modify: `crates/implant/src/runtime.rs`
- Modify: `crates/implant/src/main.rs`
- Test: `crates/implant/src/transport.rs`

**Interfaces:**
- Produces: `Transport::GSocket { forward: Arc<GSocketForward>, host, port }`
- Consumes: `GSocketConfig { secret, local_port }`

- [ ] Write failing tests for parsing, redacted descriptions, one-time process start, explicit missing-executable errors, and direct argument passing to a fake executable.
- [ ] Run `cargo test -p nw-implant transport -- --nocapture` and verify expected failures.
- [ ] Implement `GSocketForward` using `tokio::process::Command`, `kill_on_drop(true)`, `NW_GS_NETCAT`, and `-s <secret> -p <port>` arguments; connect frames to loopback after startup.
- [ ] Pass decrypted GSocket settings from `main` through `Profile` into `Transport` and keep `nw/sethost` compatible with non-GSocket transports.
- [ ] Run focused implant tests and the existing raw TCP end-to-end test.

### Task 3: Repository-backed sealed portal C2

**Files:**
- Modify: `src/c2.rs`
- Modify: `src/db/repositories.rs`
- Test: `src/c2.rs`

**Interfaces:**
- Produces: `process_sealed(repo: &Repository, psk: &[u8], wire: &[u8], protocol: &str) -> Result<Vec<u8>, C2Error>`
- Produces: HTTP `POST /c2/checkin`

- [ ] Write a failing sealed register/poll test that asserts the callback protocol, preserved task UUID, `PollReply`, and stored result.
- [ ] Run `cargo test c2::tests::sealed -- --nocapture` and verify it fails because the processor and route are absent.
- [ ] Extract repository registration/poll logic into shared functions, return `PollReply`, preserve database task IDs, and add `/c2/checkin`.
- [ ] Keep `/c2/register` and `/c2/poll` as compatibility routes calling the same core behavior.
- [ ] Run all `c2` tests and verify both legacy JSON and sealed flows pass.

### Task 4: Portal raw TCP listener

**Files:**
- Create: `src/c2_tcp.rs`
- Modify: `src/lib.rs`
- Modify: `src/config.rs`
- Modify: `src/main.rs`
- Test: `src/c2_tcp.rs`

**Interfaces:**
- Produces: `serve_tcp(repo: Repository, psk: Arc<Vec<u8>>, bind: String) -> anyhow::Result<()>`
- Consumes: `c2::process_sealed(..., "gs")`

- [ ] Write failing framing tests for valid round-trip, oversized frames, truncated frames, and GSocket protocol attribution.
- [ ] Run `cargo test c2_tcp -- --nocapture` and confirm the module/API is missing.
- [ ] Implement the bounded one-request-per-connection listener and `NAUGHTYWOLF_TCP_BIND` parsing.
- [ ] Spawn it beside Axum when configured, propagating bind failures during startup.
- [ ] Run focused tests and portal route tests.

### Task 5: Payload wizard GSocket profile

**Files:**
- Modify: `src/portal.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/payload_wizard.js`
- Modify: `static/admin.css`
- Test: `tests/payload_wizard_test.cjs`
- Test: `tests/portal_routes_test.rs`

**Interfaces:**
- Consumes: `BuildRequest.gsocket_secret` and `BuildRequest.gsocket_local_port`
- Produces: conditional wizard fields and server-side validation for protocol `gs`

- [ ] Write failing Rust and JavaScript tests for conditional fields, generated secret, validation, review copy, and secret redaction.
- [ ] Run the focused JS and portal tests and verify expected failures.
- [ ] Add conditional GSocket fields, progressive disclosure, validation messages, and metadata wiring.
- [ ] Run focused tests and inspect desktop/mobile wizard rendering.

### Task 6: Deployable GSocket adapter and documentation

**Files:**
- Modify: `Dockerfile.c2`
- Modify: `docker-compose.c2.yml`
- Modify: `.env.example.c2`
- Modify: `README.md`
- Modify: `docs/c2-quickstart.md`

**Interfaces:**
- Produces: optional Compose `gsocket` profile forwarding to private `c2:<raw-port>`

- [ ] Add a Compose configuration test/assertion that requires `GS_SECRET`, keeps the raw port unpublished, and renders the exact `gs-netcat` argument vector.
- [ ] Render Compose with `docker compose -f docker-compose.c2.yml config` and confirm the assertion initially fails.
- [ ] Add the optional service/config and document operator setup, Coolify variables, local prerequisites, secret separation, and troubleshooting.
- [ ] Re-render Compose and verify the service topology.

### Task 7: Full verification

**Files:**
- Test: workspace test suites and deployment configuration

**Interfaces:**
- Consumes: all prior tasks
- Produces: release evidence

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo test --workspace`.
- [ ] Run `node --test tests/*.cjs`.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Render Docker Compose and perform a loopback callback smoke test with a fake transparent proxy.
- [ ] Review `git diff --check` and confirm secrets do not appear in rendered HTML, metadata JSON, or logs.
