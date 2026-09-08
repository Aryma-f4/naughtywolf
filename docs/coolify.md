# Deploy NaughtyWolf on Coolify

This repository ships a Docker Compose deployment for the **web portal**: one service, SQLite, persistent storage, and a health check. The native `nw-server` remains a separate deployment described in [the C2 guide](c2-quickstart.md).

## 1. Connect the repository

Create an application from `Aryma-f4/naughtywolf`, select branch **develop**, and choose the **Docker Compose** build pack.

| Setting | Value |
| --- | --- |
| Base directory | `/` |
| Compose location | `/docker-compose.yml` |
| Service | `app` |
| Container port | `8080` |
| Health endpoint | `/healthz`, HTTP `204` |
| Persistent volume | `naughtywolf-data`, mounted at `/data` |

Assign the service an HTTPS domain such as **`https://wolf.example.com:8080`**. For Compose services, Coolify uses that port as the internal proxy destination; visitors open `https://wolf.example.com` without a port. The Compose file leaves host ports unpublished. See [Coolify's Compose routing documentation](https://coolify.io/docs/knowledge-base/docker/compose#domains).

## 2. Set the secrets

Set these two runtime environment variables in Coolify. Generate a **different** value for each with `openssl rand -hex 32`:

- `NAUGHTYWOLF_SESSION_SECRET`
- `NAUGHTYWOLF_C2_PSK`

Keep their values stable across redeploys. Missing values prevent Compose from starting; the portal also checks that the session secret is at least 32 bytes. No `.env.docker` file or PostgreSQL service is required.

`NAUGHTYWOLF_COOKIE_SECURE` defaults to `true` for HTTPS. For temporary plain-HTTP testing, explicitly set it to `false`, then restore `true` when HTTPS is enabled. `RUST_LOG` defaults to `info`.

## 3. Deploy and create the administrator

Deploy, wait for `app` to become healthy, then open its **Terminal** in Coolify and run:

```sh
naughtywolf user create --username admin --role admin
```

Enter the password at the prompt. Open `/login` on your domain and sign in. There are no default credentials, and redeploying does not reset accounts. Migrations run automatically before the portal starts.

The image supplies the HTTP health check; Compose repeats it explicitly for Coolify. [Coolify's health-check documentation](https://coolify.io/docs/knowledge-base/health-checks) explains why Compose health checks belong in the image or Compose definition.

## Storage and updates

The container runs as UID/GID `10001`. Its named volume initializes with writable ownership and stores:

| Path | Contents |
| --- | --- |
| `/data/naughtywolf.db` and its sidecars | Accounts, sessions, operations, checks, and audit records |
| `/data/evidence/` | Evidence files |
| `/data/payloads/` | Generated artifacts and metadata |

Use one replica for this SQLite deployment. Keep the same Coolify resource and its managed volume when updating. Redeploy after new commits; mounted data survives container replacement. Build caches under `/app/target` are disposable.

Before an upgrade, stop the application and back up the **entire `/data` volume**, including any SQLite WAL/SHM sidecars. Restore the complete backup to a volume owned by `10001:10001`. To roll back an incompatible schema change, restore both the previous image and its matching data backup. Do not delete the managed volume or run `docker compose down --volumes` unless you intend to discard the data.

For an existing host-directory bind mount, create and assign ownership before deployment. The default named-volume configuration avoids that manual setup. Do not mount storage over `/app`, which contains the sources used by the existing payload builder.

## Existing payload build support

The image retains Rust 1.90, Cargo, native build tools, and workspace sources at `/app`, matching the path compiled into the portal. This keeps the existing build UI available. Native Linux builds match the container architecture. First-time builds download dependencies and need outbound registry access and sufficient build memory/disk. Rust target installation is supported; cross-OS/architecture builds still require the appropriate system linker/SDK, as they do outside Docker. The default image does not bundle every cross-compilation SDK.

## Try the same container locally

```sh
export NAUGHTYWOLF_SESSION_SECRET="$(openssl rand -hex 32)"
export NAUGHTYWOLF_C2_PSK="$(openssl rand -hex 32)"
docker compose -f docker-compose.yml -f docker-compose.local.yml up --build -d
docker compose exec app naughtywolf user create --username admin --role admin
```

Open `http://127.0.0.1:8080/login`. The local overlay publishes only a loopback port and allows cookies over local HTTP. Set `NAUGHTYWOLF_HOST_PORT` if port 8080 is occupied. Save the generated secrets in your gitignored `.env` for later starts; do not regenerate them on every restart.

To verify an image before deploying (Docker and Python 3 required):

```sh
docker build -t naughtywolf:coolify-check .
python3 tests/container_smoke.py naughtywolf:coolify-check
```

The smoke test creates its own isolated container and volume, checks health/UI/CLI/toolchain access, then replaces the container to verify account, evidence, and payload-file persistence. It removes only its own randomly named test resources afterward.

## Troubleshooting

| Symptom | Resolution |
| --- | --- |
| Proxy returns 502 | Set the service domain's internal port to `8080`; keep the process bound to `0.0.0.0:8080`. |
| Sign-in does not persist | Confirm HTTPS is active when secure cookies are enabled and keep the session secret stable. |
| Database cannot be opened | Mount `/data` and check UID/GID `10001` can write there. |
| Coolify reports unhealthy | Inspect service logs and run `curl -i http://127.0.0.1:8080/healthz` inside the container; expect `204`. |
| Initial Rust build runs out of memory | Give the build host more resources or use a larger dedicated builder. |
| A cross-target build fails | Install that target's linker/SDK in a derived image; native builds use the included toolchain. |
