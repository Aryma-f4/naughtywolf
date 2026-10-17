# NaughtyWolf architecture

NaughtyWolf contains two related but separately deployed systems. The web portal is the application shown in the README screenshots and is the service deployed by the supplied Coolify Compose configuration. The native C2 crates provide a separate listener, console, and lab client.

![NaughtyWolf architecture showing the portal and native runtime as separate deployments](diagrams/naughtywolf-architecture.png)

## Portal deployment

The `naughtywolf` binary is a Tokio/Axum application. In the supplied Compose deployment, Coolify terminates public HTTPS traffic and proxies it to port `8080` in the app container.

The same process owns these portal capabilities:

- server-rendered browser UI with locally bundled CSS, JavaScript, and Anime.js;
- operations, assets, checks, evidence, reports, topology, and audit records;
- scoped DNS and HTTP HEAD observations through the recon runner;
- payload builds using the Rust workspace and toolchain included in the image;
- portal callback registration, polling, task submission, and task-result events.

SQLite data, evidence, and generated payloads live under the persistent `/data` volume in the Compose deployment. No PostgreSQL, GraphQL service, message broker, or separate frontend container is required.

```mermaid
flowchart LR
    browser[Operator browser] <-->|HTTPS / HTML / JS / SSE| edge[Coolify HTTPS edge]
    edge <-->|HTTP :8080| portal["naughtywolf\nRust + Axum"]
    portal --> ui[Web UI and topology]
    portal --> recon[Recon runner]
    recon -->|DNS and one HTTP HEAD| assets[Scoped assets]
    portal --> builder[Payload builder]
    portal --> callback["Portal callback API\n/c2/register · /c2/poll"]
    ui --> data[("/data\nSQLite · evidence · payloads")]
    recon --> data
    builder --> data
    callback --> data
```

## Native runtime

The native crates are a separate deployment path:

- `nw-server` starts the native HTTP listener and task runtime;
- `nw-console` starts its own listener and adds an in-process operator REPL;
- `nw-implant` sends sealed check-ins to `POST /c2/checkin` from an authorized lab host;
- `NW_DB` enables native SQLite persistence; without it, runtime registry and queue state are in memory.

Run `nw-server` or `nw-console` for a native runtime. The console is an alternative entry point that hosts the listener itself, rather than a remote client for an already running `nw-server` process.

```mermaid
flowchart LR
    terminal[Operator terminal] -->|console mode| native["nw-server OR nw-console\nlistener · queue · optional REPL"]
    implant["nw-implant\nauthorized lab host"] <-->|POST /c2/checkin| native
    native --> db[("Optional NW_DB\nindependent SQLite")]
```

## The live channel

Every transport in the protocol is request/response: the callback sends a sealed frame, the server answers in the same call, and anything the server wants to say is queued in `c2_tasks` until the callback's next beacon. That is deliberate — it keeps a beacon that sleeps for a minute cheap — but it makes "stop this now" wait for a timer.

The WebSocket carrier is the exception, and the exception is now load-bearing. `GET /c2/ws` speaks the same sealed-envelope protocol as `/c2/checkin`, on a socket the implant opens once and keeps:

```mermaid
sequenceDiagram
    participant I as implant (ws carrier)
    participant S as portal /c2/ws
    participant O as operator
    O->>S: cancel task
    S->>I: Live::Kill (sealed, written to the open socket)
    I->>I: kill the running child now
    I->>S: LiveEvent::Killed
    S-->>O: delivery = "live", audit: task_kill_confirmed
```

Three pieces make that work, and each has one job:

| Piece | File | Job |
| --- | --- | --- |
| `LiveSink` / `LiveRegistry` | [`src/c2_live.rs`](../src/c2_live.rs) | One writer per connected session. Pushes are sealed under that session's key and handed to the socket; a push to a session with no socket is an error, never a silent queue. |
| `checkin_ws` | [`src/c2.rs`](../src/c2.rs) | Registers the socket as the session's live channel (on the first frame that names one, including the registration acknowledgement), writes pushes to it, and routes frames the implant sends unprompted. |
| `WsLink` | [`crates/implant/src/transport.rs`](../crates/implant/src/transport.rs) | Owns the implant's socket in one task so beacons and live writes share it. A frame arriving while a beacon is outstanding is that beacon's reply; anything else is a push. |

On the implant, the beacon loop waits on the interval *and* the live channel at once (`wait_for_tick`). A kill does not sit out a sleep, and every push is handled before the interval ends, so a burst of keystrokes is not dropped between beacons. Shells live in [`crates/implant/src/shell.rs`](../crates/implant/src/shell.rs): a real PTY where the platform has one, pipes otherwise, with output pumped to the server as it happens.

The browser side is deliberately boring: [`static/shell-terminal.js`](../static/shell-terminal.js) renders the terminal (including ANSI colour) and turns key presses into terminal bytes; [`src/shell_hub.rs`](../src/shell_hub.rs) holds the per-session shell state and scrollback so a page reload reattaches to a shell that is still running; [`src/callback_workspace/shell.rs`](../src/callback_workspace/shell.rs) is the HTTP surface.

Without a live socket every one of these actions still works, with its original semantics — the kill queues and is delivered first ahead of unrelated pending work, and the UI says "queued" rather than "live".

## Integration boundary

The portal and native runtime share workspace libraries and project goals, but they are not one distributed runtime today. In particular:

| Boundary | Portal | Native runtime |
| --- | --- | --- |
| Main entry point | `naughtywolf serve` | `nw-server` or `nw-console` |
| HTTP callback routes | `/c2/register`, `/c2/poll`, `/c2/checkin`, `/c2/ws` | `/c2/checkin`, `/c2/ws` |
| Configuration prefix | `NAUGHTYWOLF_*` | `NW_*` |
| Persistence | Portal SQLite and `/data` artifacts | Optional, independent `NW_DB` SQLite |
| Accounts and sessions | Portal users and signed browser sessions | Native operator store and native sessions |
| Coolify Compose service | Included | Not included |

Building an `nw-implant` artifact in the portal does not create a live bridge between these systems. A generated artifact must be deployed only to an authorized lab host and pointed at a compatible native runtime. Any future bridge needs an explicit protocol adapter plus a deliberate identity, task, and persistence model.

## Source map

| Area | Source |
| --- | --- |
| Portal composition and startup | [`src/main.rs`](../src/main.rs) |
| Portal callback routes | [`src/c2.rs`](../src/c2.rs) |
| Live channel (server side) | [`src/c2_live.rs`](../src/c2_live.rs) |
| Attached shell state | [`src/shell_hub.rs`](../src/shell_hub.rs) |
| Shell HTTP endpoints | [`src/callback_workspace/shell.rs`](../src/callback_workspace/shell.rs) |
| Recon execution | [`src/checks/recon.rs`](../src/checks/recon.rs) |
| Payload builds | [`src/payload.rs`](../src/payload.rs) |
| Native server entry point | [`crates/server/src/main.rs`](../crates/server/src/main.rs) |
| Native listener and state | [`crates/server/src/server.rs`](../crates/server/src/server.rs) |
| Native HTTP channel | [`crates/server/src/channels.rs`](../crates/server/src/channels.rs) |
| Native console entry point | [`crates/console/src/main.rs`](../crates/console/src/main.rs) |
| Implant transport | [`crates/implant/src/transport.rs`](../crates/implant/src/transport.rs) |
| Implant shells | [`crates/implant/src/shell.rs`](../crates/implant/src/shell.rs) |
| Coolify deployment | [`docker-compose.yml`](../docker-compose.yml) |
