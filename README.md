# NaughtyWolf 🐺

Sliver C2 Web Console — modern, Neo-cyber themed operator UI for managing Sliver implants and infrastructure.

## Features

- **Lightweight SPA** — No build pipeline, no Node.js. Pure HTML/CSS/JS served by Axum.
- **Neo-cyber UI** — Dark theme with cyan/magenta accents, Cobalt Strike-style chain graph.
- **Multi-User RBAC** — Admin, Operator, Viewer roles with session-based auth.
- **Sliver Integration** — Connect to any Sliver server via gRPC mTLS.
- **Operational Pages** — Sessions, Beacons, Listeners, Payloads, Websites, Loot, Credentials, Events.
- **Chain Graph** — Visualize listener → host → implant topology.
- **Payload Generation** — Generate implants via `sliver-server` CLI integration.
- **Audit Logging** — Track all actions through the console.

## Quick Start

### Prerequisites

- PostgreSQL (local or via Docker)
- Sliver server running (for full functionality)

### Local Development

```bash
# 1. Clone and build
git clone https://github.com/Aryma-f4/naughtywolf.git
cd naughtywolf
cargo build

# 2. Setup database
docker run -d --name nw-pg -e POSTGRES_PASSWORD=naughtywolf \
  -e POSTGRES_DB=naughtywolf -e POSTGRES_USER=naughtywolf \
  -p 5432:5432 postgres:16

# 3. Create admin user
DATABASE_URL="postgres://naughtywolf:naughtywolf@localhost:5432/naughtywolf" \
NAUGHTYWOLF_SESSION_SECRET="dev-secret" \
cargo run -- user create --username admin --role admin --password naughtywolf

# 4. Run server
DATABASE_URL="postgres://naughtywolf:naughtywolf@localhost:5432/naughtywolf" \
NAUGHTYWOLF_SESSION_SECRET="dev-secret" \
NAUGHTYWOLF_BIND="0.0.0.0:8080" \
cargo run -- serve
```

Open `http://localhost:8080/login` → login with `admin` / `naughtywolf`.

### Docker Deployment

```bash
docker run -d --name nw-postgres -e POSTGRES_PASSWORD=naughtywolf \
  -e POSTGRES_DB=naughtywolf -e POSTGRES_USER=naughtywolf \
  -p 5433:5432 postgres:16

# Build the binary and run with:
SLIVER_SERVER_PATH="/path/to/sliver-server" \
NAUGHTYWOLF_BIND="0.0.0.0:8080" \
DATABASE_URL="postgres://naughtywolf:naughtywolf@host:5433/naughtywolf" \
NAUGHTYWOLF_SESSION_SECRET="production-secret" \
./naughtywolf serve
```

## Connecting to Sliver

### Generate Operator Config

```bash
# From your Sliver server:
sliver-server operator --name naughtywolf --lhost 127.0.0.1 \
  --permissions all --save /path/to/nw.cfg
```

Or via Sliver console:
```
[server] sliver > new-operator --name naughtywolf --lhost 127.0.0.1
```

### Connect via Web UI

1. Go to **Settings** (sidebar gear icon) → **Sliver Connection**
2. Enter the path to your `.cfg` operator config file
3. Click **Connect**

Or via API:
```bash
curl -X POST http://localhost:8080/api/sliver/connect \
  -H "Content-Type: application/json" \
  -d '{"config_path": "/path/to/nw.cfg"}'
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | — | PostgreSQL connection string |
| `NAUGHTYWOLF_SESSION_SECRET` | — | Session encryption key |
| `NAUGHTYWOLF_BIND` | `127.0.0.1:8080` | Server bind address |
| `SLIVER_SERVER_PATH` | `sliver-server` | Path to sliver-server binary (for payload generation) |

## API Endpoints

### Auth
- `GET /login` — Login page
- `POST /login` — Login form
- `GET /logout` — Logout

### Sliver Connection
- `GET /api/sliver/status` — Connection status
- `POST /api/sliver/connect` — Connect (body: `{"config_path": "..."}`)
- `POST /api/sliver/disconnect` — Disconnect

### Data
- `GET /api/dashboard/stats` — Dashboard statistics
- `GET /api/sessions` — Active sessions
- `GET /api/beacons` — Active beacons
- `GET /api/listeners` — Active listeners/jobs
- `POST /api/listeners` — Start listener (body: `{"protocol":"http|mtls","host":"0.0.0.0","port":8080}`)
- `POST /api/listeners/kill/{id}` — Kill listener
- `GET /api/payloads` — List implant builds
- `POST /api/payloads/generate` — Generate implant (body: `{"name":"test","goos":"linux","goarch":"amd64","format":"exe","is_beacon":false,"protocol":"mtls","lhost":"127.0.0.1","lport":443}`)
- `GET /api/websites` — Websites
- `GET /api/loot` — Loot
- `GET /api/creds` — Credentials

### Admin
- `GET /api/users` — List users (admin only)
- `GET /api/events` — SSE event stream

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────────┐
│  Browser    │────▶│  NaughtyWolf │────▶│  PostgreSQL  │
│  (SPA)      │     │  (Axum)      │     │  (Sessions)  │
└─────────────┘     └──────┬───────┘     └──────────────┘
                           │
                           ▼
                    ┌──────────────┐     ┌──────────────┐
                    │  Sliver      │◀────│  sliver-     │
                    │  Server      │     │  server CLI  │
                    │  (gRPC)      │     │  (generate)  │
                    └──────────────┘     └──────────────┘
```

## Tech Stack

- **Backend:** Rust, Axum, tonic (gRPC), sqlx, tower-sessions
- **Frontend:** Vanilla HTML/CSS/JS, hash-routing SPA, SVG graph
- **TLS:** rustls with aws-lc-rs provider
- **Database:** PostgreSQL (via sqlx migrations)

## License

Internal tool — authorized security testing only.
