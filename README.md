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
cargo test
```

## License

Internal tool for authorized security-lab use only.
