# NaughtyWolf

**A sharper instinct. A clearer view.**

A Rust workspace for authorized security labs, with a local web portal for operations, assets, check history, evidence, and reports. Keep the scope of your work and the records behind it in one place.

The interface pairs a dark red theme with editorial typography, a custom wolf mark, and responsive layouts. Menu navigation updates the page content while keeping the header and navigation in place.

[Quick start](#quick-start) · [Screenshots](#screenshots) · [Configuration](#configuration) · [Development](#development) · [Native C2 guide](docs/c2-quickstart.md)

![NaughtyWolf dashboard with the red theme, scoped record counts, and workflow overview](docs/screenshots/dashboard-desktop.jpg)

## What is included?

| Component | What it does | Start here |
| --- | --- | --- |
| **Web portal** — `naughtywolf` | Browser UI, local accounts, SQLite records, evidence, reports, and callback/task routes. | [Quick start](#quick-start) |
| **Native server** — `nw-server` | Standalone C2 listener with optional SQLite persistence. | [Native C2 guide](docs/c2-quickstart.md) |
| **Native console** — `nw-console` | Interactive operator console that hosts its own listener. | [Native C2 guide](docs/c2-quickstart.md) |
| **Lab client** — `nw-implant` | Client component for the native C2 workspace. | [Native C2 guide](docs/c2-quickstart.md) |

The portal and native C2 packages have separate configuration and account stores. **You only need the web portal to run the interface shown here.**

## Features

- **Operation records:** define a purpose, track assets, and keep related lab records together.
- **Scoped overview:** dashboard counts come from persisted records visible to the signed-in user.
- **Evidence and reporting:** inspect stored artifacts and produce printable operation summaries.
- **Traceability:** an append-only audit trail records operation, asset, and account changes.
- **Local access control:** Admin, Operator, and Viewer roles, password hashing, signed sessions, and CSRF-protected forms.
- **Evidence verification:** downloads check the stored path, file type, length, and SHA-256 digest.
- **Responsive interface:** desktop navigation, a scrollable mobile menu, locally bundled icons and scripts, keyboard focus indicators, and reduced-motion support.
- **No frontend build pipeline:** Rust renders the HTML; CSS and JavaScript ship with the binary.

## Quick start

### 1. Check the prerequisites

Install a current stable Rust toolchain with Cargo and the native compiler/linker required by your platform. The commands below use a POSIX shell and OpenSSL to generate a session secret.

The portal uses **SQLite**. It does not require a separate database server, Node.js, or npm.

### 2. Clone and build the portal

```bash
git clone https://github.com/Aryma-f4/naughtywolf.git
cd naughtywolf
cargo build -p naughtywolf --bin naughtywolf
```

### 3. Configure your local workspace

Run these commands in the same terminal you will use to create accounts and start the server:

```bash
export NAUGHTYWOLF_DATABASE_URL='sqlite:naughtywolf.db?mode=rwc'
export NAUGHTYWOLF_BIND='127.0.0.1:8080'
export NAUGHTYWOLF_SESSION_SECRET="$(openssl rand -hex 32)"
export NAUGHTYWOLF_COOKIE_SECURE=false
```

For repeat starts, save these settings—including the generated secret—in your gitignored `.env` file. The portal loads `.env` on startup; existing shell environment variables take precedence. Keep the session secret stable across restarts to preserve signed sessions.

The root `.env.example` still contains legacy PostgreSQL/Sliver settings. Use the `NAUGHTYWOLF_*` settings above for this portal.

### 4. Create your administrator

```bash
cargo run -p naughtywolf -- user create --username admin --role admin
```

Enter a password at the terminal prompt. There is no default portal login. The database and schema are initialized automatically.

### 5. Start the server

```bash
cargo run -p naughtywolf -- serve
```

Open **[http://127.0.0.1:8080/login](http://127.0.0.1:8080/login)** and sign in with your new account.

The server exposes a health endpoint at `/healthz`; a healthy process returns HTTP `204`.

## Your first workflow

1. **Define an operation.** As an Admin, open **Operations → Create operation** and enter its name and purpose.
2. **Record the assets.** Use **Add asset** on an operation to register its scoped targets.
3. **Review the records.** Inspect inventory, available check history, and evidence associated with your work.
4. **Read the summary.** Open **Reports** for operation summaries and **Audit** to trace recorded changes.

A fresh workspace starts with zero operations, assets, checks, and evidence. Creating an account can already produce an audit record. Dashboard figures are stored counts, not sample analytics.

## Screenshots

Captured from the running application using an isolated local preview account. These images show the red theme and a fresh workspace; they do not contain live operation data.

### Operator access

![Desktop sign-in page with red accents and the NaughtyWolf identity](docs/screenshots/login-desktop.jpg)

### Mobile workspace

The navigation scrolls horizontally, and the dashboard reflows for a narrow screen.

<img src="docs/screenshots/dashboard-mobile.jpg" alt="NaughtyWolf dashboard at a 390-pixel mobile viewport" width="390">

The desktop dashboard is shown at the top of this README. Original screenshots are in [`docs/screenshots`](docs/screenshots).

## Pages and access

| Page | Route | Purpose |
| --- | --- | --- |
| Dashboard | `/dashboard` | Visible operation, asset, check, evidence, and audit counts. |
| Operations | `/operations` | Operation records and asset creation. |
| Assets | `/inventory` | Inventory for visible operations. |
| Checks | `/checks` | Recorded check-run history. |
| Evidence | `/evidence` | Artifact metadata and verified downloads. |
| Reports | `/reports` | Printable summaries from stored operation records. |
| Audit | `/audit` | Recorded actions and outcomes. |
| Callbacks | `/callbacks` | Callback records and task details. |
| Services | `/services` | Service records. |
| Eventing | `/eventing` | Event-rule configuration. |
| Event feed | `/events` | Workspace event records. |
| Payloads | `/payloads` | Build forms and generated artifacts. |
| Search | `/search` | Search available workspace records. |
| Administration | `/admin/users` | Local account roles and enabled state. |

Callbacks, services, eventing, the event feed, payloads, and search require an Operator or Admin account. Administration requires an Admin account.

| Role | Portal permissions |
| --- | --- |
| **Admin** | View all operation records, create operations, and manage local accounts. |
| **Operator** | Work with assigned operations, add scoped assets, and access operator pages. |
| **Viewer** | Read assigned operation records; no mutation or operator-only controls. |

Operation-related records for non-Admin users are limited by operation membership. A newly created Operator or Viewer account does not automatically gain access to every operation. In the administration UI, administrators cannot disable their own account or change their own role.

## Configuration

Portal settings are read from the process environment or `.env` in the working directory.

| Variable | Default | Purpose |
| --- | --- | --- |
| `NAUGHTYWOLF_DATABASE_URL` | `sqlite:naughtywolf.db?mode=rwc` | SQLite connection URL. Use the same value for account commands and the server. |
| `NAUGHTYWOLF_BIND` | `127.0.0.1:8080` | HTTP bind address and port. |
| `NAUGHTYWOLF_SESSION_SECRET` | Required | Secret of at least 32 bytes for signing sessions. |
| `NAUGHTYWOLF_COOKIE_SECURE` | `false` | Set to `true` when serving the portal through HTTPS. |
| `NAUGHTYWOLF_EVIDENCE_DIR` | `evidence` | Root directory for evidence files. |
| `NAUGHTYWOLF_C2_PSK` | `dev-psk-change-me` | Shared key for the portal's C2 routes. Replace before using those routes. |
| `RUST_LOG` | `info,naughtywolf=debug` | Logging filter. |

Relative database and evidence paths resolve from the working directory. Keep that directory consistent between runs.

The `NW_*` variables used by the native C2 packages are documented in the [separate guide](docs/c2-quickstart.md).

## Account management

Run these from a terminal configured for the same database as the server:

```bash
# Create an account; the password is prompted interactively.
cargo run -p naughtywolf -- user create --username reviewer --role viewer

# List accounts.
cargo run -p naughtywolf -- user list

# Reset an account password interactively.
cargo run -p naughtywolf -- user reset-password --username reviewer

# Disable an account.
cargo run -p naughtywolf -- user disable --username reviewer
```

For CI, account creation and password reset also accept `--password-stdin`. Use an interactive prompt for normal local administration.

## Development

### UI changes require a restart

The active portal renders templates from `src/portal/templates.rs` and embeds `static/admin.css`, `static/admin.js`, and `static/anime.min.js` at compile time.

After editing them, stop the running server and run:

```bash
cargo run -p naughtywolf -- serve
```

Cargo rebuilds changed assets. Then refresh the browser once. Subsequent menu navigation updates the main content without replaying navbar animations.

The older `static/app.html`, `static/app.js`, and `static/style.css` files are not the active portal UI.

### Validation

```bash
# Check Rust formatting across the workspace.
cargo fmt --all --check

# Check JavaScript syntax (requires Node.js for this check only).
node --check static/admin.js

# Run the portal route, rendering, and access-control tests.
cargo test -p naughtywolf --test portal_routes_test

# Run the full workspace test suite.
cargo test --workspace
```

### Repository map

```text
src/
  main.rs                 Portal startup, sessions, and login
  portal.rs               Portal routes and handlers
  portal/templates.rs     Active HTML templates
  auth/                   Passwords, sessions, and roles
  db/                     SQLite models and repositories
  evidence.rs             Evidence storage and verification
static/
  admin.css               Active portal theme and responsive layouts
  admin.js                Content navigation and interactions
  anime.min.js            Bundled animation library
migrations/               Database schema migrations
crates/                   Native server, console, client, and shared code
tests/                    Portal and data-layer integration tests
docs/
  screenshots/            Screenshots used in this README
  c2-quickstart.md         Native C2 setup
```

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| Server reports a missing or short session secret | Set `NAUGHTYWOLF_SESSION_SECRET` to at least 32 bytes in the environment or `.env`. |
| Login fails after creating an account | Confirm the CLI and server use the same database URL and working directory. |
| Login does not persist on local HTTP | Use `NAUGHTYWOLF_COOKIE_SECURE=false` for local HTTP; use `true` with HTTPS. |
| UI changes do not appear | Rebuild and restart the server, then refresh the page. CSS and JavaScript are embedded in the binary. |
| Port 8080 is already in use | Stop the existing instance or set `NAUGHTYWOLF_BIND=127.0.0.1:8082` and open that port. |
| An Operator or Viewer sees no operations | Check operation membership; account creation alone does not grant access to existing operations. |
| You expect a browser UI after starting `nw-server` | Start the `naughtywolf` portal. The native server is a separate entry point. |
| Root Docker setup expects PostgreSQL or missing build files | The root Docker recipe targets older infrastructure. Use the native portal quick start above. |

## Intended use

NaughtyWolf is intended for authorized security-lab work on systems you own or have explicit permission to test. The quick start binds the portal to loopback for local development. For a remote deployment, serve it through HTTPS, enable secure cookies, replace development secrets, and protect the database and evidence directory with operating-system access controls.
