# NaughtyWolf 🐺

NaughtyWolf is a standalone local portal for authorized security-lab records. It keeps operations, inventory, non-destructive check history, evidence metadata, reports, local accounts, and an append-only audit trail in SQLite.

## Features

- Server-rendered, responsive pages with no frontend build step.
- Local Admin, Operator, and Viewer roles with signed sessions.
- Operation membership scoping for non-Admin users.
- Audited operation, asset, and account changes.
- Bounded evidence storage with path, file type, length, and SHA-256 verification before download.
- Printable summaries built only from stored operation records.

## Local development

### Prerequisites

- A current stable Rust toolchain.
- SQLite support supplied through the Rust dependencies; no separate database service is required.

Clone the repository and build it:

```bash
git clone https://github.com/Aryma-f4/naughtywolf.git
cd naughtywolf
cargo build
```

Create the first local administrator. The command prompts for the account password without placing it in shell history:

```bash
NAUGHTYWOLF_DATABASE_URL='sqlite:naughtywolf.db?mode=rwc' \
NAUGHTYWOLF_SESSION_SECRET='replace-with-at-least-32-random-bytes' \
NAUGHTYWOLF_COOKIE_SECURE=false \
cargo run -- user create --username admin --role admin
```

Start the local server:

```bash
NAUGHTYWOLF_DATABASE_URL='sqlite:naughtywolf.db?mode=rwc' \
NAUGHTYWOLF_SESSION_SECRET='replace-with-at-least-32-random-bytes' \
NAUGHTYWOLF_COOKIE_SECURE=false \
cargo run -- serve
```

Open [http://127.0.0.1:8080/login](http://127.0.0.1:8080/login) and sign in with the account you created.

This HTTP configuration is for local development only. Production deployments require an HTTPS origin and `NAUGHTYWOLF_COOKIE_SECURE=true`. Use a unique random session secret of at least 32 bytes and protect the database and evidence directory with operating-system access controls.

## C2 quickstart

The legacy `naughtywolf` package above is a local security-lab records portal. The C2 workspace is separate: `nw-server`, `nw-console`, and `nw-implant` live under `crates/`. Use it only in an authorized lab against systems you own or are explicitly permitted to test.

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

For the server container, copy the local template and replace the development PSK. The container always listens on `0.0.0.0:8081`; set `NW_HOST_PORT` in `.env.c2` to choose its host-side port. In M1, `NW_CALLBACK_HOST` is reserved and inert: the server does not consume, advertise, log, or distribute it. Then use the exact Compose commands below:

```bash
cp .env.example.c2 .env.c2
# edit .env.c2 for your authorized lab
docker compose --env-file .env.c2 -f docker-compose.c2.yml up --build
docker compose --env-file .env.c2 -f docker-compose.c2.yml down
```

## Configuration

| Variable | Default | Description |
| --- | --- | --- |
| `NAUGHTYWOLF_DATABASE_URL` | `sqlite:naughtywolf.db?mode=rwc` | Local SQLite database URL. |
| `NAUGHTYWOLF_BIND` | `127.0.0.1:8080` | Server bind address. |
| `NAUGHTYWOLF_EVIDENCE_DIR` | `evidence` | Root directory for generated evidence files. |
| `NAUGHTYWOLF_SESSION_SECRET` | none | Required signed-session secret of at least 32 bytes. |
| `NAUGHTYWOLF_COOKIE_SECURE` | `false` | Set to `true` when served from an HTTPS origin. |

## Local account commands

```bash
cargo run -- user create --username reviewer --role viewer
cargo run -- user list
cargo run -- user disable --username reviewer
```

Supply the same `NAUGHTYWOLF_DATABASE_URL` for every command that should use the same database. Account roles and disabled state can also be changed by an Admin from `/admin/users`; those changes are recorded in the audit trail. An administrator cannot change their own role or disable their own account.

## Portal pages

| Route | Purpose |
| --- | --- |
| `/dashboard` | Counts for records visible to the current user. |
| `/operations` | Authorized operation records and scoped asset creation. |
| `/inventory` | Scoped inventory. |
| `/checks` | Scoped check-run history. |
| `/audit` | Scoped append-only audit history. |
| `/evidence` | Safe evidence metadata and verified downloads. |
| `/reports` | Printable operation summaries from stored records. |
| `/admin/users` | Admin-only local account controls. |

All non-Admin reads are limited to operations where the current user is a member. Evidence downloads resolve that scope before opening a file, then re-check the generated relative path, regular-file status, recorded length, and SHA-256 digest.

## Verification

```bash
cargo fmt --check
cargo test --workspace
```

## License

Internal tool for authorized security-lab use only.
