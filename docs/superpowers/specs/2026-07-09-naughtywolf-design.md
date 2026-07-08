# NaughtyWolf — Sliver Web GUI Design Specification

## Overview

NaughtyWolf is a standalone Rust Axum web application that provides an authorized operator/admin GUI for Sliver C2. It runs alongside a Sliver server on the same VPS, discovers local operator configurations, and exposes curated Sliver functionality through a modern web interface.

**Key principles:**
- NaughtyWolf is frontend-only: it invokes existing Sliver gRPC APIs, not reimplements implant operations.
- No raw OS shell execution from web UI.
- No stealth/evasion automation.
- No persistence automation.
- No unguarded implant command bridge.
- All state-changing actions are authenticated, authorized, and audited.

## Architecture

```text
browser
  ↓ HTTPS/session cookie
NaughtyWolf Axum web app
  ├─ web UI routes/static assets
  ├─ JSON API routes
  ├─ auth/session/RBAC middleware
  ├─ audit logger
  ├─ PostgreSQL app database
  └─ Sliver adapter
       ├─ discovers local Sliver operator configs
       ├─ opens mTLS + bearer gRPC connection
       ├─ calls curated Sliver RPCs
       └─ bridges Sliver Events() stream to UI
            ↓
local Sliver server (already installed on VPS)
```

## Stack

| Layer | Technology |
|-------|-----------|
| Web framework | Rust Axum 0.8 |
| Database | PostgreSQL |
| DB driver | sqlx (async, compile-time checked queries) |
| Session auth | Axum middleware, session cookies |
| Password hashing | Argon2 |
| Sliver gRPC | tonic with Prost (protobuf stubs from Sliver's protobuf/) |
| Event stream | SSE (Server-Sent Events) |
| Template/UI | server-rendered HTML or static SPA |
| CI/Tooling | cargo, sqlx-cli, cargo-watch |

## PostgreSQL Schema

```sql
CREATE TYPE user_role AS ENUM ('admin', 'operator', 'viewer');

CREATE TABLE users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username text UNIQUE NOT NULL,
    password_hash text NOT NULL,
    role user_role NOT NULL DEFAULT 'operator',
    disabled bool NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sliver_profiles (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE,
    config_path text NOT NULL,
    operator_name text NOT NULL,
    lhost text NOT NULL,
    lport integer NOT NULL,
    fingerprint text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_sliver_profiles (
    user_id uuid REFERENCES users(id) ON DELETE CASCADE,
    profile_id uuid REFERENCES sliver_profiles(id) ON DELETE CASCADE,
    default_profile bool NOT NULL DEFAULT false,
    PRIMARY KEY (user_id, profile_id)
);

CREATE TABLE audit_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid REFERENCES users(id),
    profile_id uuid REFERENCES sliver_profiles(id),
    action text NOT NULL,
    target_type text NOT NULL,
    target_id text,
    parameter_summary jsonb,
    result_status text NOT NULL,
    result_ref text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ui_preferences (
    user_id uuid REFERENCES users(id) ON DELETE CASCADE,
    key text NOT NULL,
    value jsonb NOT NULL,
    PRIMARY KEY (user_id, key)
);
```

## Module Layout

```
naughtywolf/
  Cargo.toml
  .env.example              # DATABASE_URL, NAUGHTYWOLF_BIND, etc.
  migrations/
  src/
    main.rs                 # boot Axum, config, tracing
    config.rs               # env config parsing
    db/
      mod.rs                # PostgreSQL pool, migrations runner
      models.rs             # user, role, profile, audit structs
    auth/
      mod.rs                # login, logout, session
      password.rs           # Argon2 hashing/verification
      rbac.rs               # role checks
      middleware.rs          # session auth extractor
    sliver/
      mod.rs                # adapter boundary
      profiles.rs           # discover ~/.sliver-client/configs/*.cfg
      connection.rs         # tonic mTLS gRPC client with bearer token
      events.rs             # Sliver Events() stream → SSE bridge
    actions/
      listeners.rs          # curated listener operations
      payloads.rs           # curated profile/build operations
      websites.rs           # curated website content operations
      sessions.rs           # curated session operations
      beacons.ts            # curated beacon operations
      loot.rs               # curated loot browsing
      creds.rs              # curated credentials browsing
    web/
      routes.rs             # HTML/static page routes
      api.rs                # JSON API routes
      filters.rs            # request validation/extraction
      templates.rs          # template rendering or SPA handler
      sse.rs                # Server-Sent Events endpoint
    audit.rs                # audit event logger
    cli/
      mod.rs                # CLI subcommands
      users.rs              # user create/reset-password/disable/list
      profiles.rs           # profile scan/assign/list
```

## CLI (Local-Only User Registration)

```bash
naughtywolf user create --username alice --role admin
naughtywolf user reset-password --username alice
naughtywolf user disable --username alice
naughtywolf user list

naughtywolf profile scan                           # discover Sliver configs
naughtywolf profile assign --username alice --profile alice_127.0.0.1
naughtywolf profile list
```

## Code Map

All routes are behind the session auth middleware. Auth wall applies to both HTML pages and JSON APIs. The CLI runs locally on the VPS and authenticates through socket or direct DB connection. The `web/templates.rs` handles rendering with a consistent dark/light-blue themed layout. Static assets (CSS/JS) are embedded via `rust_embed` or `axum-embed`.

```
src/main.rs             → reads Config, initializes DB pool, builds Router with state
src/config.rs           → struct Config { database_url, bind, sliver_config_dir, session_secret, etc. }
src/db/mod.rs           → run_migrations(), get_pool()
src/db/models.rs        → User, SliverProfile, AuditEvent structs + DB queries
src/auth/mod.rs         → login handler, logout handler, session cookie management
src/auth/password.rs    → hash_password(), verify_password()
src/auth/rbac.rs        → require_role() / check_permission()
src/auth/middleware.rs   → axum middleware, extracts user from session cookie, injects into request
src/sliver/mod.rs       → SliverAdapter struct
src/sliver/profiles.rs  → scan_profiles(), parse_profile(), Profile struct
src/sliver/connection.rs → open_connection(), SliverRPCClient wrapper, reconnect logic
src/sliver/events.rs    → event loop calling Events RPC, normalizing to internal types
src/actions/listeners.rs → start/stop listener workflows via Sliver gRPC
src/actions/payloads.rs → curated payload build forms → gRPC
src/actions/websites.rs → website content management
src/actions/sessions.rs → session listing, info, curated actions
src/actions/beacons.rs  → beacon listing, info, curated tasks
src/actions/loot.rs     → loot file browsing
src/actions/creds.rs    → credential browsing
src/web/routes.rs       → axum Router with page routes (/dashboard, /sessions, etc.)
src/web/api.rs          → axum Router with JSON API routes (/api/sessions, etc.)
src/web/filters.rs      → validation/parsing for API and form inputs
src/web/templates.rs    → render HTML or serve SPA (dark/light-blue theme)
src/web/sse.rs          → /events endpoint, client subscribes to event stream
src/audit.rs            → record_audit_event() stored in PostgreSQL
src/cli/mod.rs          → clap CLI definition
src/cli/users.rs        → subcommands for user management
src/cli/profiles.rs     → subcommands for profile mapping
```

## Configuration

```env
DATABASE_URL=postgres://postgres:root@localhost:5432/naughtywolf
NAUGHTYWOLF_BIND=127.0.0.1:8080
NAUGHTYWOLF_SESSION_SECRET=generate-a-long-random-string
SLIVER_CONFIG_DIR=$HOME/.sliver-client/configs
```

Set via environment variables or `.env` file (`.env` is gitignored). The schema is validated at startup by `src/config.rs` with descriptive error messages for missing required fields. The session secret must be a cryptographically random string of at least 64 bytes.

## UI Theme

**Palette:**

| Token | Hex |
|-------|-----|
| Background | `#070b14` |
| Surface | `#0d1524` |
| Surface Raised | `#111d31` |
| Border | `#1f3554` |
| Primary Blue | `#38bdf8` |
| Primary Blue Soft | `#7dd3fc` |
| Accent Cyan | `#22d3ee` |
| Text Main | `#e5f3ff` |
| Text Muted | `#8aa4bd` |
| Danger | `#fb7185` |
| Warning | `#facc15` |
| Success | `#34d399` |

**Layout:**
```
┌─────────────────────────────────────────────────────────┐
│ top bar | NaughtyWolf | active Sliver profile | user menu│
├───────────────┬─────────────────────────────────────────┤
│ left nav      │ main panel                              │
│ Dashboard     │ cards/tables/forms                      │
│ Sessions      │                                         │
│ Beacons       │                                         │
│ Listeners     │                                         │
│ Jobs          │                                         │
│ Payloads      │                                         │
│ Websites      │                                         │
│ Loot          │                                         │
│ Credentials   │                                         │
│ Events        │                                         │
│ Audit         │                                         │
│ Admin         │                                         │
└───────────────┴─────────────────────────────────────────┘
```

## Pages

- **Login**: dark theme, wolf/Sliver-inspired mark, no registration link/button.
- **Dashboard**: active listeners, session/beacon counts, recent events, quick stats.
- **Sessions**: table with details drawer, action forms (interact, info, download, screenshot, etc.).
- **Beacons**: table with beacon details, pending tasks, results view.
- **Listeners/Jobs**: start/stop listener forms, job status table.
- **Payloads**: profile management, build request forms.
- **Websites**: content metadata tables, upload/update workflows.
- **Loot**: searchable file table, download.
- **Credentials**: table with cred type, user, host.
- **Events**: color-coded live event stream (SSE).
- **Audit**: queryable log of NaughtyWolf state-changing actions.
- **Admin**: web user management, role assignment, Sliver profile mapping, connection status.

## RBAC

| Role | Permissions |
|------|------------|
| `viewer` | read-only dashboard, sessions, beacons, events, audit (cannot act) |
| `operator` | all viewer + curated session/beacon/listener/payload actions |
| `admin` | all operator + web user management, profile mapping, CLI access |

Viewer role shows action buttons as disabled with "Read-only — contact admin" tooltip hint.

## Event Stream

```
Sliver Events() gRPC stream
  → NaughtyWolf event worker (src/sliver/events.rs)
  → normalized internal Event type
  → channel → SSE endpoint (/api/events)
  → browser EventSource
  → dashboard widgets update
```

Events are typed and color-coded. The SSE endpoint includes heartbeat pings every 30 seconds to detect and recover from stale connections. Browser clients reconnect automatically.

## Data Flow

1. User submits form or clicks action in browser.
2. HTTP request reaches Axum route handler.
3. Session middleware extracts user from cookie.
4. RBAC middleware checks user role against required permission.
5. Handler validates parameters via typed forms/JSON.
6. Audit event is persisted to PostgreSQL before the action executes.
7. Sliver adapter translates to gRPC call with mapped operator profile credentials.
8. Result/task ID is returned to the browser as JSON.
9. If the action changes state on the Sliver side, the Events stream pushes an update.

## Testing

- Unit tests for config parsing, auth, password hashing, RBAC rules.
- Sliver profile parser tests with fixture `.cfg` files (stripped of real credentials).
- API route tests with in-memory SQLite or separate PostgreSQL test database, mocked Sliver adapter.
- Audit logging is verified for each state-changing action.
- CLI command tests create/verify/tidy test database state.
- A mocked gRPC client returns fixture responses for all action modules; an optional integration suite hits a configured Sliver test server for teams that maintain one.
- Front-end tests (where applicable) cover login flow, role-based action visibility, and event stream rendering.

## Acceptance Gates

- `cargo fmt --check` passes.
- `cargo clippy` passes (no warnings as errors, but any clippy issues must be reviewed).
- `cargo test` passes.
- Migrations apply cleanly to a fresh PostgreSQL database.
- App boots with valid `DATABASE_URL` and reaches login page.
- CLI can create first admin user.
- Login succeeds with newly created credentials.
- No registration route or page exists.
- Sliver profile scan discovers valid configs.
- At least one read-only page renders successfully (must verify with actual Sliver connection or a bounded mock).
- Console output and logs do not contain raw secrets, tokens, or password hashes.

## Deliverables

The core deliverable is an authenticated web operator console for controlling and monitoring a Sliver C2 server. NaughtyWolf exposes curated Sliver functionality through a safe, audited interface. The project is standalone and lives in a separate directory tree (e.g. `../naughtywolf`), not modifying the upstream Sliver repository. It is licensed independently under the MIT license.

## Non-Goals

- Implant generation or modification.
- Sliver server replacement.
- Unauthenticated access to any endpoint.
- Arbitrary remote shell or implant passthrough without typed, approved actions.
- Automated implant stealth, evasion, or persistence capabilities.

## Future Scope

- OIDC/SSO authentication support.
- Multi-host deployment with remote profile import.
- Automated artifact management (deletion, archival, expiry).
- Notification integration (webhooks, Slack).
- WebSocket fallback when SSE is not available or behind certain reverse proxies.
