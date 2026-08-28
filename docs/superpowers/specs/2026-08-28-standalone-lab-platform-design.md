# Standalone Authorized Security Lab Platform — Design

## Purpose

NaughtyWolf will be refocused from a Sliver web console into a lightweight,
standalone platform for managing authorized security-lab work. It will run as
one Rust binary with an embedded SQLite database and no dependency on Sliver,
gRPC, protobuf, Docker, or an external database server.

The phase-one product manages approved operations and assets, executes a small
set of built-in defensive checks, retains evidence, and creates auditable
reports. It is designed for local lab and certification practice.

## Non-goals

The platform will not generate payloads, act as a command-and-control server,
or include stealth, evasion, persistence, remote agent, listener, beacon,
implant, or arbitrary-command functionality. It will not accept user-uploaded
scripts or construct shell commands from UI input.

## Architecture

```text
Browser
  └─ HTTPS / session cookie
       └─ NaughtyWolf single Rust binary
            ├─ HTML, CSS, and small vanilla-JS UI
            ├─ authenticated JSON API
            ├─ RBAC and operation-scope policy
            ├─ operations, assets, evidence, and reporting services
            ├─ built-in defensive check registry and runner
            ├─ append-only audit service
            └─ SQLite database
```

The server binds to `127.0.0.1` by default. A configured bind address is
supported for a trusted lab network. Transport security is the responsibility
of a local reverse proxy when it is exposed beyond localhost.

The Rust backend remains Axum-based. The database moves from SQLx/PostgreSQL
to SQLite. The frontend stays dependency-free: static assets served by Axum,
without Node.js or a build pipeline.

## Components

### Identity and RBAC

The application has `admin`, `operator`, and `viewer` roles.

- Admins manage users, settings, operations, assets, and built-in checks.
- Operators can read assigned operations, manage assets in them, and start or
  cancel permitted checks for in-scope assets.
- Viewers have read-only access to operations to which they are assigned.

All write routes require authentication, a role check, and an operation-scope
check where applicable. Passwords use Argon2 and sessions use secure,
HttpOnly, SameSite cookies.

### Operations and inventory

An operation has a name, explicit purpose, status (`planned`, `active`,
`closed`), optional start/end times, members, and a written scope note. An
asset belongs to exactly one operation in phase one and carries a display name,
asset type, owner, approved address/hostname, status, tags, and notes.

Checks can only be started against an active asset within an active operation.
Closing an operation prevents new runs while preserving its evidence and audit
history.

### Built-in defensive checks

Checks are compiled into the application and registered in a static catalog.
Each catalog item declares its role requirement, typed input schema, time
limit, output limit, and result schema. Phase one ships only non-destructive
local or network-configuration checks appropriate for an explicitly approved
asset. It does not execute a user-supplied script, invoke a shell, scan broad
ranges, exploit services, or change a target.

The runner receives validated values, invokes an internal Rust implementation,
and records a structured result. It has explicit states: `queued`, `running`,
`succeeded`, `failed`, and `cancelled`. Timeout, cancellation, and output-size
limits are enforced by the runner.

### Evidence, audit, and reports

Every completed run creates an immutable result and optional evidence blob.
Evidence retains content type, byte length, SHA-256 checksum, creation time,
and a safe path within the application data directory. The database stores
metadata rather than unbounded output when limits are exceeded.

Audit records are append-only. They capture actor, action, object type and ID,
operation ID when applicable, validated parameter summary, outcome, timestamp,
and a correlation ID. Audit entries are written for logins, role-sensitive
requests, state changes, and check lifecycle events.

Reports provide a dated summary of an operation, its assets, check results,
evidence references, and audit timeline. JSON and CSV exports are generated on
demand and do not contain password hashes, sessions, or unredacted secrets.

## Data model

SQLite migrations create these primary tables:

- `users` and `sessions`
- `operations` and `operation_members`
- `assets` and `asset_tags`
- `checks` and `check_runs`
- `evidence`
- `audit_events`

Foreign keys are enabled. Database writes that change domain state and create
the corresponding audit event occur in one transaction. Check-result storage
is likewise transactional: a failed persistence step does not mark a run as
successfully completed.

## UI

The UI uses a restrained field-console visual language rather than a generic
dashboard: charcoal and stone surfaces, system typography, strong hierarchy,
and limited status color. Dense data appears in well-spaced tables with a
detail drawer, not card grids. Decorative glows, faux-terminal effects, and
empty metric cards are excluded.

The primary navigation is a centered floating bottom dock. It contains concise
icon-and-label entries for Overview, Operations, Assets, Checks, Evidence,
Audit, Reports, and Settings. The active destination is a clearly labeled
pill; the header contains the page title, active operation indicator, search,
and user menu. A separate primary action sits beside the dock. Content reserves
bottom safe space so the dock never obscures controls or logs.

Risk-sensitive actions present a confirmation dialog that names the operation,
asset, and check. Keyboard navigation and visible focus states are required.
The layout is laptop-first and remains usable on tablets and narrow screens.

## Failure handling

User-facing errors state what failed and the safe recovery action. Detailed
diagnostics go to structured tracing and audit records without exposing secrets
or internal paths. A missing or locked database makes the application fail
closed. A failed, timed-out, or cancelled check records its terminal state and
never appears as a successful result.

## Verification

The implementation will include:

- unit tests for RBAC, scope enforcement, validation, audit construction, and
  check-run state transitions;
- SQLite migration and transactional-integrity tests;
- API integration tests for authentication, operation/asset lifecycle, and
  permitted and denied check runs;
- runner tests for timeout, cancellation, output limits, and no-shell
  invocation; and
- browser smoke tests for the floating dock and primary page navigation.

## Migration direction

The existing Sliver adapter, protobuf build pipeline, payload/listener/session
features, and their routes are removed rather than retained behind flags. The
rewrite begins by establishing the SQLite core and identity boundary, then adds
operations, assets, audit, checks, reports, and the new UI in that order.
