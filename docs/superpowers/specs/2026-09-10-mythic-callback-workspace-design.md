# Mythic-Style Callback Workspace Design

**Date:** 2026-09-10  
**Status:** Proposed  
**Scope:** Native NaughtyWolf callbacks on Linux and Windows

## Objective

Replace the current callback detail page with a persistent operator workspace inspired by Mythic. The workspace must provide tasking, process control, filesystem control, file transfer, and trustworthy callback metadata without changing NaughtyWolf into a React/GraphQL application.

The server remains the source of truth. Reloading, leaving a callback, reconnecting an SSE stream, or restarting the portal must not erase task history, browser state, or transfer progress.

## Confirmed Current Defects

1. `list_tasks_for_session` selects `r.exit_code`, but `C2TaskWithResult` expects `result_exit_code`. SQLx returns a missing-column error.
2. The callback page and task JSON route convert that database error into an empty list with `unwrap_or_default`, making persisted tasks appear deleted.
3. Implant discovery reads only exported `HOSTNAME`/`COMPUTERNAME` and `USER`/`USERNAME` variables. On the production Ubuntu process, `HOSTNAME` was not exported, so the registration stored `unknown`.
4. The production callback stopped checking in at `2026-09-10T05:43:58.044Z`. Its recorded PID no longer exists. Tasks created after that time correctly remain queued but the UI does not explain why.
5. The native HTTP check-in path ignores file chunks and upload acknowledgements. The implant contains partial transfer primitives, but native callback transfer is not end-to-end functional.

## Reference and Adaptation

Mythic's tasking workspace uses an authoritative initial query, incremental subscription updates, ID-based merging, older-task pagination, filters, expandable task responses, and a persistent command input. NaughtyWolf will adapt those interaction patterns to its Rust, server-rendered HTML, SQLite, and vanilla JavaScript stack.

Porting Mythic's React components or GraphQL schema is explicitly out of scope. The goal is equivalent operator behavior and information hierarchy, not source-level duplication.

## Workspace Information Architecture

The callback route remains `/callbacks/{session_id}` and renders a single workspace with four tabs:

### Tasking

- Chronological command/response timeline with stable task IDs.
- State pills for Queued, Delivered, Processing, Completed, Error, and Cancelled. “Queued” is the UI label for the existing persisted `pending` state.
- Expandable command parameters, stdout, stderr, exit code, timestamps, and operator identity.
- Search and filters for state, command, operator, time, and errors.
- Cursor-based “load older” pagination; no fixed history cutoff.
- Sticky command dock with server-backed history and keyboard navigation.
- Retry creates a new task linked to the original task.
- Cancel removes a queued task or sends a cancellation request for a running task.
- When a callback is offline, queued tasks remain visible with “Waiting for callback check-in”.

### Processes

- Explicit refresh task that returns a structured process snapshot.
- Searchable and sortable columns: PID, parent PID, name, executable, user, architecture, CPU, memory, and start time where the OS exposes them.
- Process detail drawer showing the latest known values and snapshot time.
- Kill action with confirmation. The resulting control task is visible in Tasking and the table refreshes only after a successful response.
- Failure messages distinguish permission denial, nonexistent PID, unsupported data, timeout, and offline callback.

### Files

- Path bar, breadcrumbs, parent navigation, and directory/file table.
- Structured listing fields: name, absolute path, kind, size, modified time, permissions, and owner where available.
- Actions: refresh, upload, download, mkdir, rename/move, and recursive/non-recursive delete.
- Destructive actions require confirmation that names the exact remote path.
- Upload and download show byte progress, checksum, status, and resumability.
- The selected tab and current path are represented in the URL so browser navigation and reload preserve context.

### Metadata

- Hostname, username, OS/version, architecture, implant PID, process path, local address, transport, callback endpoint, callback ID, first seen, last seen, sleep interval, jitter, and implant version.
- Online/offline/stale state is computed from `last_seen` and the configured beacon interval, rather than trusting a permanently stored `active` flag.
- Values unavailable on an OS are shown as “Unavailable”, not `unknown` unless the implant explicitly reports that literal value.

## Server Architecture

### Authoritative task lifecycle

SQLite owns the task lifecycle:

`pending -> delivered -> processing -> completed | error | cancelled`

- Enqueue inserts a durable pending task before returning to the browser.
- Inclusion in a poll reply transitions it to delivered, but delivered tasks are resent until the implant acknowledges their IDs.
- The implant acknowledges accepted task IDs on the next poll, transitioning them to processing.
- The implant tracks accepted/running task IDs and ignores a redelivered duplicate, giving task execution at-least-once delivery without concurrent duplicate execution.
- A result atomically stores stdout, stderr, exit code, completion time, and final state.
- Cancellation of an unclaimed task is immediate. Cancellation of an accepted task creates a control request and records the eventual result.
- All state-changing repository methods use transactions and append an audit event in the same transaction.

The existing SQL projection is corrected with an explicit `AS result_exit_code`. Repository errors propagate to the route and produce a visible error response; they are never converted into an empty history.

### API surface

- `GET /api/callbacks/{id}/tasks?limit=&before=` returns authoritative paginated tasks.
- `POST /api/callbacks/{id}/tasks` enqueues a validated command.
- `POST /api/callbacks/{id}/tasks/{task_id}/cancel` cancels a task.
- `GET /api/callbacks/{id}/events` streams task and transfer updates over authenticated SSE.
- `POST /api/callbacks/{id}/processes/refresh` and `/processes/{pid}/kill` enqueue typed process tasks.
- `POST /api/callbacks/{id}/files/list`, `/mkdir`, `/move`, and `/delete` enqueue typed filesystem tasks.
- `POST /api/callbacks/{id}/files/upload` accepts an authenticated multipart upload and creates a transfer plus task.
- `POST /api/callbacks/{id}/files/download` creates a remote-to-server transfer task.
- `GET /api/callbacks/{id}/transfers/{transfer_id}/download` serves only completed, checksum-verified downloads to authorized operators.

Every mutation requires operator-or-admin role, callback visibility, and CSRF validation. Callback visibility checks occur before disclosing whether a task, path, PID, or transfer exists.

### Realtime reconciliation

The initial paginated API response is authoritative. SSE carries incremental task and transfer snapshots. The browser upserts by stable ID and refetches the current page after any reconnect, preventing missed events from creating stale state. A disconnected stream changes the UI indicator to “Reconnecting” without discarding displayed data.

## Protocol and Implant Commands

Structured commands use versioned JSON results rather than parsing localized shell output:

- `nw/process-list`
- `nw/process-kill <pid>`
- `nw/fs-list <absolute-path>`
- `nw/fs-stat <absolute-path>`
- `nw/fs-mkdir <absolute-path>`
- `nw/fs-move <source> <destination>`
- `nw/fs-delete <absolute-path> <recursive:false|true>`
- `nw/download <absolute-path> <transfer-id>`
- `nw/upload <absolute-path> <transfer-id>`

Each structured result includes a schema identifier such as `nw.process-list.v1` or `nw.fs-list.v1`. Unknown schema versions remain readable as raw task output and do not break the task timeline.

Registration metadata gains optional, backward-compatible fields for OS version, executable path, local address, implant version, interval, and jitter. Older payloads continue to register. Hostname and username use OS APIs first, then environment and platform-specific file/account fallbacks.

## Cross-Platform Behavior

Process enumeration and termination use a maintained Rust system-information abstraction with small platform adapters for fields that differ between Linux and Windows. Unsupported fields remain nullable. Kill means the platform's normal terminate operation; no injection or memory manipulation is part of this workspace.

Filesystem listing and mutations use Rust filesystem APIs. Paths are transported as strings without shell interpolation. Windows drive roots and UNC paths are represented explicitly; Linux absolute paths begin at `/`. Operations run with the implant process's existing OS permissions and must return permission failures without retrying as another identity.

## File Transfer

A new durable transfer record stores transfer ID, callback ID, task ID, direction, remote path, server storage key, expected size, received bytes, checksum, status, error, and timestamps.

Protocol chunks are bound to a transfer ID and task ID. The server writes downloads to a temporary file in the protected NaughtyWolf data volume, validates offsets and configured size limits, fsyncs, verifies SHA-256, then atomically publishes the completed artifact. Uploads read from an authenticated server-side staging file and resume from the implant's acknowledged offset.

Only one upload and one download may be active per callback initially. Additional transfers remain durably queued. Temporary files are not downloadable and are cleaned only after a terminal state plus retention period.

## Persistence Model

New migrations add:

- task operator, parent-task, updated-at, and cancellation fields;
- callback metadata and beacon timing fields;
- durable file-transfer records;
- latest structured process and filesystem snapshot records, updated transactionally from successful typed task results.

Task results remain the immutable source record. Process and filesystem views are projections of successful structured results and always display their snapshot timestamp. A portal restart reconstructs every tab from SQLite and protected transfer storage.

## Safety and Audit

- Exact PID/path, callback ID, operator, action, task ID, outcome, and timestamp are audited.
- Kill and filesystem mutation dialogs show the exact target and cannot be bypassed by pressing Enter on the underlying page.
- Arguments are passed directly to typed Rust functions, never concatenated into a shell command.
- Result and directory sizes are bounded; UI truncation never truncates the persisted downloadable result.
- Transfer filenames never determine server paths; server storage uses generated IDs.
- Secrets and session keys are excluded from metadata APIs, HTML, SSE, logs, and audit details.

## Error Handling

- Database failures return an explicit retryable portal error and are logged with internal context.
- Offline callbacks accept queued tasks but clearly show that no delivery has occurred.
- Invalid or stale PIDs and paths produce terminal task errors.
- A restarted implant registers a new callback unless session restoration is later introduced; the previous callback and all its task history remain accessible as an offline record.
- A server restart resumes transfers from durable offsets and preserves pending/delivered tasks; unacknowledged delivered tasks are eligible for redelivery.
- An implant disconnect during a destructive task leaves the task in its last acknowledged state until timeout or a later result; it is not falsely marked successful.

## Testing and Verification

### Repository and migration tests

- Reproduce the current `result_exit_code` projection failure before fixing it.
- Prove history survives repository and application restart.
- Cover every task transition, cancellation race, pagination boundary, audit transaction, and transfer resume.

### Implant tests

- Metadata discovery returns a real hostname when environment variables are absent.
- Structured process and filesystem JSON round-trips on Linux and Windows.
- Filesystem tests operate only inside temporary directories.
- Process-kill tests spawn and terminate only a dedicated test child.
- Transfer tests cover resume, duplicate chunks, out-of-order chunks, size limits, checksum mismatch, and atomic publication.

### Portal tests

- Reload restores tasks, selected tab/path, snapshots, and transfer progress.
- SSE reconnect reconciles missed state without duplicates.
- Offline queue messaging, filters, pagination, confirmations, RBAC, CSRF, output escaping, and audit behavior are covered.

### CI and production verification

- Linux runs formatting, linting, unit, integration, and end-to-end tests.
- Windows runs implant metadata/process/filesystem/transfer tests.
- Deployment remains gated on the Linux suite and container health check.
- Production verification confirms the deployed SHA, healthy container, HTTPS response, persistent task query, and one authorized non-destructive callback task before destructive controls are used.

## Rollout Order

1. Correct task projection/error propagation and add lifecycle regression coverage.
2. Implement metadata discovery and accurate liveness display.
3. Introduce paginated task API, unified SSE reconciliation, and Tasking tab.
4. Add structured process protocol, implant handlers, API, and Processes tab.
5. Add structured filesystem operations and Files tab.
6. Complete durable upload/download transport and transfer UI.
7. Add Windows CI coverage, full workspace regression suite, and production deployment verification.

Each stage is backward compatible with the previous deployed payload where possible. UI controls requiring a newer implant are disabled with a clear version-capability message.

## Acceptance Criteria

- Returning to any callback shows all persisted tasks and results without duplicates.
- An offline callback visibly retains queued tasks and explains why they are waiting.
- A Linux or Windows callback reports a real hostname when the OS can provide one.
- Process refresh and confirmed kill work through typed, audited tasks.
- File navigation, upload, verified download, mkdir, move, and confirmed delete work through typed, audited tasks.
- Reload and portal restart preserve task state, current browser context, and transfer progress.
- All controls enforce callback visibility, operator role, CSRF, output escaping, and exact-target audit records.
- Linux and Windows CI suites pass, and the VPS deploys the exact tested commit with a healthy HTTPS service.
