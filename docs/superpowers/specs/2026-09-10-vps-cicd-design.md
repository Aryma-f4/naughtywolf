# VPS CI/CD Deployment Design

## Goal

Deploy the `develop` branch of NaughtyWolf to the Ubuntu VPS at
`43.156.147.197` after every successful GitHub push, serve the portal at
`https://gateofbabylon.space`, and make both the HTTPS callback endpoint and
authenticated raw TCP callback listener reachable.

## Architecture

GitHub Actions connects to the VPS with a dedicated deployment SSH key and
runs a versioned deployment script from the repository. The script maintains
a clean checkout at `/opt/naughtywolf`, preserves the VPS-only `.env`, builds
the Compose images, starts the application, and fails unless the container and
public health endpoint become healthy.

The application joins the existing external `coolify` Docker network. Labels
on the application container let the existing Traefik proxy request a
Let's Encrypt certificate and route `gateofbabylon.space` to port 8080 inside
the container. Port 4630 is explicitly published on the VPS for authenticated
raw TCP callbacks. The GSocket sidecar continues to use the same private raw
listener.

## Components

- `.github/workflows/deploy-vps.yml`: validates the deployment configuration,
  pins the VPS host key, and invokes the remote deployment script on pushes to
  `develop` or manual dispatches.
- `deploy/vps/docker-compose.yml`: production override containing Traefik
  routing labels, the external Coolify network, and raw TCP port publication.
- `deploy/vps/deploy.sh`: locked, fail-fast remote deployment with checkout,
  Compose build/start, container health, and public HTTPS verification.
- VPS `/opt/naughtywolf/.env`: stable application secrets generated once with
  mode 0600; never copied into GitHub or committed.
- GitHub Actions secrets: dedicated private deployment key and pinned VPS host
  key. The public key is installed for `ubuntu` on the VPS.

## Deployment Flow

1. A push reaches `develop`.
2. GitHub Actions checks out that exact commit and validates Compose rendering
   and shell syntax.
3. The action opens a pinned SSH connection and uploads the exact Git commit
   SHA as the deployment target.
4. The VPS script fetches that SHA, checks it out in `/opt/naughtywolf`, and
   runs Docker Compose with the repository base file plus the VPS override.
5. The script waits for the application health check and verifies
   `https://gateofbabylon.space/healthz` returns HTTP 204.

Only one workflow deployment may run at a time. A newer push cancels an older
queued deployment. A failed build leaves the previously running container in
place because Compose replacement happens only after the new image builds.

## Security and Persistence

The workflow does not contain application secrets or the user's primary SSH
key. The dedicated key is limited to deployment use and stored as a GitHub
Actions secret. SSH host verification uses a pinned public host key.

SQLite, evidence, and generated payloads stay in the existing named volume.
The deployment never runs `docker compose down --volumes`. HTTPS cookies remain
enabled. TCP port 4630 accepts only the existing sealed, authenticated
NaughtyWolf protocol.

## Verification

- Shell syntax and Compose rendering pass locally and in GitHub Actions.
- DNS resolves `gateofbabylon.space` to `43.156.147.197`.
- Traefik obtains a valid certificate and `/healthz` returns HTTP 204 over
  HTTPS.
- The application container reports healthy after deployment.
- Port 4630 accepts a TCP connection from outside the VPS and the application
  logs show the listener bound successfully.
- A manual GitHub workflow run completes, proving the same path used by future
  pushes.
