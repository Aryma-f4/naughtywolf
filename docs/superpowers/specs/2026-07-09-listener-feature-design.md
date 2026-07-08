# NaughtyWolf Listener Feature Design

Date: 2026-07-09

## Goal

NaughtyWolf will let authenticated operators create, view, and stop Sliver listeners from the web GUI. The feature must support the current typed Sliver gRPC adapter and also support the local `sliver-client` child-process integration requested for workflows where Sliver is already configured locally.

## Scope

### Included

- Listener list refresh from Sliver jobs.
- Listener creation for initial protocols:
  - `mtls`
  - `http`
- Listener stop/kill by Sliver job ID.
- Operator/Admin-only create and kill actions.
- Viewer read-only access.
- Hybrid backend:
  - gRPC primary when a Sliver profile is connected through NaughtyWolf.
  - `sliver-client` CLI fallback only when no gRPC connection exists.
- Clear status messages for missing CLI binary, Sliver errors, validation errors, and timeouts.

### Excluded for this iteration

- HTTPS certificate upload/ACME management.
- DNS listener-specific advanced options.
- Long-poll timeout/jitter controls.
- Listener persistence outside Sliver's own job list.
- Real-time streaming updates beyond manual/automatic table refresh.

## Existing Context

Current code already provides most typed Sliver operations:

- `src/actions/listeners.rs`
  - `list_jobs(conn)` lists Sliver jobs/listeners via gRPC.
  - `start_mtls(conn, host, port)` starts mTLS listener via gRPC.
  - `start_http(conn, host, port, domain)` starts HTTP listener via gRPC.
  - `kill_job(conn, job_id)` stops listener job via gRPC.
- `src/web/api.rs`
  - `GET /api/listeners` lists listeners only when gRPC is connected.
- `src/web/routes.rs`
  - `/listeners` page renders a table and placeholder `+ New Listener` button.
- `src/sliver/cli.rs`
  - Payload generation already spawns `sliver-client` as a child process.
  - Listener feature should reuse and generalize this child-process helper instead of duplicating process code.

## Architecture

### Backend Components

#### `src/actions/listeners.rs`

Add request/response and validation types:

- `ListenerProtocol`
  - enum values: `Mtls`, `Http`
  - parses API strings `mtls` and `http`
- `CreateListenerRequest`
  - `protocol: String`
  - `host: String`
  - `port: u32`
  - `domain: Option<String>`
- `ListenerActionResponse`
  - `success: bool`
  - `message: String`
  - `job: Option<JobResponse>`
  - `source: String` (`grpc` or `cli`)

Add orchestration functions:

- `validate_create_request(req) -> Result<ValidatedCreateListener, String>`
- `start_listener_grpc(conn, validated) -> Result<JobResponse, String>`
- `start_listener_cli(validated) -> Result<ListenerActionResponse, String>`
- `kill_listener_cli(job_id) -> Result<ListenerActionResponse, String>`

The existing gRPC primitives remain focused and small. Hybrid choice happens in API handlers where `AppState` knows whether a gRPC connection exists.

#### `src/sliver/cli.rs`

Generalize child-process execution:

- Add `run_console_commands(commands: &[String], timeout_secs: u64) -> Result<ConsoleOutput, String>`.
- Keep `generate()` as a payload-specific wrapper using `run_console_commands()`.
- Add listener command builders:
  - `build_mtls_listener_command(host, port)`
  - `build_http_listener_command(host, port, domain)`
  - `build_kill_job_command(job_id)`

CLI fallback uses the same binary selection as payloads:

- `SLIVER_CLIENT_PATH` if set.
- Otherwise `sliver-client` from `PATH`.

CLI fallback returns a success message and `source: "cli"`. It may not always know the final Sliver job ID because Sliver CLI output can vary by version. After a successful CLI create, UI refreshes the list; if gRPC is not connected, the table can show the CLI success message but cannot guarantee live job enumeration.

### API Components

Add routes in `api_routes()`:

- `POST /api/listeners`
  - Operator/Admin only.
  - Body: `CreateListenerRequest`.
  - Behavior:
    1. Validate input.
    2. If gRPC connection exists, use gRPC.
    3. If no gRPC connection exists, use CLI fallback.
    4. Return `ListenerActionResponse`.
- `DELETE /api/listeners/:id`
  - Operator/Admin only.
  - Behavior:
    1. If gRPC connection exists, call `kill_job`.
    2. If no gRPC connection exists, call CLI fallback.
    3. Return `ListenerActionResponse`.

Update existing `GET /api/listeners`:

- If gRPC connection exists, return active Sliver jobs as now.
- If no gRPC connection exists, return an empty list instead of `503` so the GUI can still load and show the create form.
- Do not invent listener rows from local state.

### UI Components

Update `/listeners` page:

- Replace placeholder `+ New Listener` alert with an inline create form/card.
- Fields:
  - Protocol: `mTLS` or `HTTP`
  - Host: default `0.0.0.0`
  - Port: default `8888` for mTLS, `80` for HTTP
  - Domain: visible for HTTP, optional
- Table columns:
  - ID
  - Protocol
  - Bind Address
  - Profile/Source
  - Status
  - Actions
- Operator/Admin sees:
  - create form
  - kill button per row when an ID is available
- Viewer sees:
  - table only
  - read-only message instead of form/actions

## Data Flow

### Create Listener

1. User submits listener form in `/listeners`.
2. Browser sends `POST /api/listeners` JSON.
3. API checks role via `AuthenticatedUserGuard`.
4. API validates protocol, host, and port.
5. API locks `state.sliver` briefly:
   - If connected, calls typed gRPC action.
   - If disconnected, drops lock and calls local CLI fallback.
6. API returns `ListenerActionResponse`.
7. Browser shows success/error message.
8. Browser reloads listener table.

### Kill Listener

1. User clicks Kill button on a listener row.
2. Browser sends `DELETE /api/listeners/{id}`.
3. API checks role.
4. API uses gRPC kill when connected; CLI kill otherwise.
5. Browser shows result and reloads table.

## Error Handling Rules

- Empty host: reject before calling Sliver.
- Port `0`: reject before calling Sliver.
- Unsupported protocol: reject before calling Sliver.
- Viewer create/kill: return forbidden.
- gRPC connected but call fails: return gRPC failure. Do not fallback to CLI because that could start a duplicate listener against a different local Sliver config.
- No gRPC connection and no `sliver-client`: return clear CLI setup error.
- CLI timeout: return timeout failure and log it.
- CLI nonzero exit: include stdout/stderr summary in server logs; send concise failure message to UI.

## Security and Authorization

This is a local authorized C2 management UI feature. It must preserve current RBAC:

- Admin: list, create, kill.
- Operator: list, create, kill.
- Viewer: list only.

Do not expose arbitrary command execution through listener APIs. CLI mode must build commands from validated structured fields only.

## Testing Plan

### Unit Tests

- `ListenerProtocol` parsing accepts `mtls` and `http`.
- Parsing rejects unsupported protocols.
- Validation rejects empty host.
- Validation rejects port `0`.
- CLI command builders produce expected Sliver console commands.
- CLI helper reports missing binary errors clearly.

### Integration/Build Checks

- `cargo check`
- `cargo test`
- `cargo clippy`

Manual browser check:

- `/listeners` renders create form for Admin/Operator.
- `/listeners` hides actions for Viewer.
- `GET /api/listeners` returns `[]` when disconnected instead of breaking the page.
- `POST /api/listeners` shows useful error if `sliver-client` is missing and no gRPC profile is connected.

## Implementation Order

1. Generalize `src/sliver/cli.rs` with reusable console command runner.
2. Add listener request/validation/CLI command builder code in `src/actions/listeners.rs`.
3. Add `POST /api/listeners` and `DELETE /api/listeners/:id` handlers.
4. Update `GET /api/listeners` disconnected behavior.
5. Replace `/listeners` placeholder UI with create form and kill actions.
6. Add unit tests.
7. Run format, clippy, and tests.

## Acceptance Criteria

- Admin/Operator can create mTLS and HTTP listeners from the GUI.
- Admin/Operator can stop active listeners from the GUI when job IDs are available.
- Viewer cannot create or stop listeners.
- GUI loads even when Sliver gRPC is not connected.
- CLI fallback attempts local `sliver-client` only when no gRPC connection exists.
- Missing CLI binary produces a clear actionable error.
- No arbitrary command strings from user input reach the shell.
- `cargo test` and `cargo clippy` pass.
