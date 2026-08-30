# Docker Containerization — NaughtyWolf

## Approach

App + PostgreSQL in docker-compose. Sliver server remains external.

## Files

### `Dockerfile` — multi-stage Rust build

```
Builder stage:
  - Base: rust:latest (Debian bookworm)
  - Install protoc (required by tonic_build/build.rs at compile time)
  - Copy: Cargo.toml, build.rs, protobuf/, src/, static/, migrations/
  - Build: cargo build --release
  - Output: target/release/naughtywolf

Runtime stage:
  - Base: debian:bookworm-slim
  - Copy binary from builder
  - No Rust toolchain, no protoc, no protobuf sources
  - USER nobody
```

No sqlx-cli needed — migrations run at startup via embedded code (`db::run_migrations`).

### `docker-compose.yml`

```yaml
services:
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: naughtywolf
      POSTGRES_PASSWORD: naughtywolf
      POSTGRES_DB: naughtywolf
    volumes: pgdata volume
    healthcheck: pg_isready -U naughtywolf

  app:
    build: .
    ports: 8080:8080
    depends_on: db (condition: service_healthy)
    env_file: .env.docker
```

### `.env.docker`

Overrides `.env` with compose-appropriate values:
- `DATABASE_URL=postgres://naughtywolf:naughtywolf@db:5432/naughtywolf`
- `NAUGHTYWOLF_BIND=0.0.0.0:8080`
- `NAUGHTYWOLF_SESSION_SECRET` — production secret (user must set)
- `SLIVER_CONFIG_DIR=/configs` — mount point for operator configs
- `RUST_LOG=info,naughtywolf=debug`

### `.dockerignore`

```
target/
.git/
.env
*.md
docs/
tests/
```

## Build

```bash
docker compose build
docker compose up -d
# First run — create admin user:
docker compose exec app ./naughtywolf user create --username admin --role admin --password <pw>
```

## Connect Sliver

Mount operator config into container and connect via UI at Settings > Sliver Connection, or bind-mount `/configs`.

## Dev vs Production

- Dev: `docker compose up`, local sliver server
- Prod: swap `.env.docker` values, add reverse proxy for TLS termination
