# Docker Containerization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Containerize NaughtyWolf with multi-stage Docker build + docker-compose (app + PostgreSQL).

**Architecture:** Multi-stage Dockerfile (builder + runtime slim images). docker-compose.yml runs PostgreSQL 16 Alpine and the built app. Sliver server stays external — user connects via Settings UI.

**Tech Stack:** Docker, Docker Compose, Rust, PostgreSQL

## Global Constraints

- Runtime image: `debian:bookworm-slim` (glibc compat for tonic gRPC)
- Builder image: `rust:latest` (includes protoc via apt)
- DB password default: `naughtywolf` (dev — override in .env.docker)
- Session secret must be set by user (no default in production)
- Static assets baked into binary via `include_str!` — no runtime volumes needed
- Migrations run at startup in Rust code — no sqlx-cli needed at runtime
- Binary listens on `0.0.0.0:8080` (not `127.0.0.1` — container networking)

---

### Task 1: Dockerfile

**Files:**
- Create: `Dockerfile`

**Interfaces:**
- Consumes: project source, Cargo.toml, build.rs, protobuf/, static/, migrations/
- Produces: `/usr/local/bin/naughtywolf` binary in runtime image

- [ ] **Step 1: Write Dockerfile**

```dockerfile
# syntax=docker/dockerfile:1
# Builder stage
FROM rust:latest AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml build.rs ./
COPY protobuf ./protobuf/
COPY src ./src/
COPY static ./static/
COPY migrations ./migrations/

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/naughtywolf /usr/local/bin/naughtywolf

USER nobody
EXPOSE 8080

ENTRYPOINT ["naughtywolf", "serve"]
```

- [ ] **Step 2: Commit**

```bash
git add Dockerfile
git commit -m "feat: add multi-stage Dockerfile"
```

### Task 2: docker-compose.yml

**Files:**
- Create: `docker-compose.yml`

**Interfaces:**
- Consumes: Dockerfile, .env.docker
- Produces: runnable `docker compose up` — app on :8080, pg on :5432

- [ ] **Step 1: Write docker-compose.yml**

```yaml
services:
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: naughtywolf
      POSTGRES_PASSWORD: naughtywolf
      POSTGRES_DB: naughtywolf
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U naughtywolf"]
      interval: 5s
      timeout: 5s
      retries: 5
    ports:
      - "5432:5432"
    restart: unless-stopped

  app:
    build: .
    ports:
      - "8080:8080"
    env_file: .env.docker
    depends_on:
      db:
        condition: service_healthy
    restart: unless-stopped

volumes:
  pgdata:
```

- [ ] **Step 2: Commit**

```bash
git add docker-compose.yml
git commit -m "feat: add docker-compose with app + PostgreSQL"
```

### Task 3: .env.docker

**Files:**
- Create: `.env.docker`

**Interfaces:**
- Consumes: docker-compose.yml service names (db, app)
- Produces: environment for `app` service

- [ ] **Step 1: Write .env.docker**

```bash
DATABASE_URL=postgres://naughtywolf:naughtywolf@db:5432/naughtywolf
NAUGHTYWOLF_BIND=0.0.0.0:8080
NAUGHTYWOLF_SESSION_SECRET=replace-with-random-64-char-hex-string
SLIVER_CONFIG_DIR=/configs
RUST_LOG=info,naughtywolf=debug
```

- [ ] **Step 2: Commit**

```bash
git add .env.docker
git commit -m "feat: add .env.docker for compose environment"
```

### Task 4: .dockerignore

**Files:**
- Create: `.dockerignore`

**Interfaces:**
- Consumes: project root
- Produces: lean Docker build context

- [ ] **Step 1: Write .dockerignore**

```
target/
.git/
.env
*.md
docs/
tests/
```

- [ ] **Step 2: Commit**

```bash
git add .dockerignore
git commit -m "chore: add .dockerignore"
```

### Task 5: Build and verify

**Files:** (none — verification)

- [ ] **Step 1: Build the Docker image**

```bash
docker compose build
```

Expected: builds naughtywolf:latest image without errors.

- [ ] **Step 2: Start services**

```bash
docker compose up -d
sleep 5
docker compose ps
```

Expected: both `db` and `app` services running.

- [ ] **Step 3: Verify app responds**

```bash
curl -s -w "\n%{http_code}" http://127.0.0.1:8080/
```

Expected: 303 redirect (root → /app).

- [ ] **Step 4: Verify login page**

```bash
curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8080/login
```

Expected: 200 OK (login HTML page).

- [ ] **Step 5: Create admin user via compose exec**

```bash
docker compose exec -e NAUGHTYWOLF_SESSION_SECRET=replace-with-random-64-char-hex-string -e DATABASE_URL=postgres://naughtywolf:naughtywolf@db:5432/naughtywolf app ./naughtywolf user create --username admin --role admin --password naughtywolf
```

Expected: User created successfully.

- [ ] **Step 6: Tear down**

```bash
docker compose down -v
```
