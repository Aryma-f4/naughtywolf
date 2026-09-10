# Native C2 quickstart

## GSocket payloads from the portal

The web portal can generate a `gs` transport payload that appears directly in `/callbacks`. Set `GSOCKET_SECRET` and `NAUGHTYWOLF_C2_PSK` to different random values in Coolify, deploy `docker-compose.yml`, then select **GSocket tunnel** in the payload wizard and enter the same GSocket secret.

The generated implant expects `gs-netcat` on the authorized lab host's `PATH` (or at `NW_GS_NETCAT`). It starts a local forward with direct process arguments and connects NaughtyWolf's sealed TCP frames through it. The implant does not download GSocket, invoke a shell, or install persistence. The Compose sidecar follows GSocket's documented TCP-forwarding topology and keeps portal port `4630` private.

The `naughtywolf` web portal and the native C2 workspace are separate entry points. This guide covers the native packages: `nw-server`, `nw-console`, and `nw-implant` live under `crates/`. Use it only in an authorized lab against systems you own or are explicitly permitted to test.

Build the C2 binaries:

```bash
cargo build -p nw-server -p nw-console -p nw-implant
```

Start a native server-only listener with a matching bind address and PSK. Set `NW_DB` to retain sessions and tasks across restarts. Set each development implant's `NW_ENDPOINT` explicitly, generate a configured implant as shown below, or use `redirect <host>` for an existing implant.

```bash
NW_BIND=127.0.0.1:8081 \
NW_CALLBACK_HOST=http://127.0.0.1:8081 \
NW_PSK='replace-with-a-lab-psk' \
NW_DB='naughtywolf-c2.db' \
cargo run -p nw-server
```

For an interactive native listener, run the console instead (it hosts the listener and operator REPL):

```bash
NW_BIND=127.0.0.1:8081 \
NW_PSK='replace-with-a-lab-psk' \
NW_DB='naughtywolf-c2.db' \
NW_ADMIN_PASSWORD='replace-with-a-unique-long-password' \
cargo run -p nw-console
```

`NW_ADMIN_PASSWORD` is used only when the operator database is empty. Remove it after the initial admin is created. Later console starts prompt for an existing operator. Viewers can inspect sessions and jobs; operators and admins can task sessions. Every accepted or denied REPL command is written to `c2_audit`.

In another terminal, run an implant with the same PSK and the callback endpoint:

```bash
NW_ENDPOINT=http://127.0.0.1:8081 \
NW_PSK='replace-with-a-lab-psk' \
cargo run -p nw-implant
```

At the console, use `sessions`, then `interact <session-id>`. Use `shell <cmd> [args...]` for an authorized test command, or `redirect <host>` to queue `nw/sethost` and change the focused implant's endpoint.

Generate a release implant with an encrypted build-time profile:

```bash
cargo run -p nw-console -- generate \
  --endpoint http://127.0.0.1:8081 \
  --psk 'replace-with-a-lab-psk' \
  --interval-ms 5000 \
  --jitter-ms 1000 \
  --output ./nw-implant-lab
```

For the server container, copy the local template and replace the development PSK. The container always listens on `0.0.0.0:8081`; set `NW_HOST_PORT` in `.env.c2` to choose its host-side port. `NW_CALLBACK_HOST` is currently reserved: the server does not consume, advertise, log, or distribute it. The supplied Compose recipe does not configure `NW_DB` or a database volume, so it does not retain sessions across container recreation. These commands target the native server, not the web portal:

```bash
cp .env.example.c2 .env.c2
# edit .env.c2 for your authorized lab
docker compose --env-file .env.c2 -f docker-compose.c2.yml up --build
docker compose --env-file .env.c2 -f docker-compose.c2.yml down
```

## How this relates to the web portal

The native server and console use `NW_*` settings and their own operator store. The web portal uses `NAUGHTYWOLF_*` settings and its own local accounts. Starting `nw-server` does not start the browser UI, and a portal account does not create a native console account.

See [the main README](../README.md) for portal setup, screenshots, and configuration.
