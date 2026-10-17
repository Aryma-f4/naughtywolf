#!/usr/bin/env bash
# Bootstrap a NaughtyWolf VPS deployment.
#
# Idempotent: safe to re-run. Installs Docker if absent, opens only the ports
# that must be public, generates secrets, and starts the stack.
#
#   sudo ./deploy/deploy.sh gateofbabylon.space
#
# Assumes a fresh Ubuntu host with an A/AAAA record already pointing here.

set -euo pipefail

DOMAIN="${1:-}"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$REPO_DIR/.env"

if [[ -z "$DOMAIN" ]]; then
	echo "usage: $0 <your-domain>" >&2
	exit 2
fi

log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- preflight

[[ $EUID -eq 0 ]] || die "run as root (sudo)"
command -v docker >/dev/null 2>&1 || die "docker is not installed"
docker compose version >/dev/null 2>&1 || die "docker compose v2 is required"

# The public IP must actually resolve here, or certificate issuance will fail
# and the operator console would be unreachable.
PUBLIC_IP="$(curl -fsS --max-time 10 https://api.ipify.org || true)"
log "host public address: ${PUBLIC_IP:-unknown}"
log "requested domain:    $DOMAIN"
if [[ -n "$PUBLIC_IP" ]]; then
	RESOLVED="$(getent ahostsv4 "$DOMAIN" 2>/dev/null | awk 'NR==1{print $1}' || true)"
	if [[ "$RESOLVED" != "$PUBLIC_IP" ]]; then
		log "WARNING: $DOMAIN resolves to ${RESOLVED:-nothing}, not $PUBLIC_IP."
		log "         Certificate issuance will fail until the A record matches."
		read -r -p "Continue anyway? [y/N] " reply
		[[ "$reply" == "y" || "$reply" == "Y" ]] || die "aborted"
	fi
fi

# ------------------------------------------------------------------- secrets

# Generate once, then reuse: rotating the session secret logs every operator
# out and rotating the C2 PSK silently invalidates every built payload.
gen_secret() { head -c 48 /dev/urandom | base64 | tr -d '\n=+/' | cut -c1-48; }

if [[ -f "$ENV_FILE" ]]; then
	log "reusing existing $ENV_FILE (secrets preserved)"
else
	log "generating secrets into $ENV_FILE"
	umask 077
	cat >"$ENV_FILE" <<EOF
# NaughtyWolf deployment secrets. Mode 0600, never commit this file.
GATEWAY_DOMAIN=$DOMAIN
NAUGHTYWOLF_SESSION_SECRET=$(gen_secret)
NAUGHTYWOLF_C2_PSK=$(gen_secret)
NAUGHTYWOLF_ADMIN_PASSWORD=$(gen_secret)
# Separate key for the headless C2 so a payload built for one cannot be
# pointed at the other by accident.
NW_C2_PSK=$(gen_secret)
NW_CALLBACK_HOST=https://$DOMAIN/c2b
NAUGHTYWOLF_COOKIE_SECURE=true
EOF
fi

# The initial admin password is only consulted when the operator table is
# empty, so surface it now rather than letting it get lost.
if grep -q '^NAUGHTYWOLF_ADMIN_PASSWORD=' "$ENV_FILE"; then
	ADMIN_PW="$(grep '^NAUGHTYWOLF_ADMIN_PASSWORD=' "$ENV_FILE" | cut -d= -f2-)"
	log "initial admin username: admin"
	log "initial admin password: $ADMIN_PW"
	log "store it now; it is not shown again and only applies on first boot."
fi

# --------------------------------------------------------------------- docker

if ! docker info >/dev/null 2>&1; then
	log "starting docker"
	systemctl enable --now docker
fi

# --------------------------------------------------------------------- build

log "building images (first run compiles the workspace twice; be patient)"
cd "$REPO_DIR"
docker compose --env-file "$ENV_FILE" -f deploy/compose.vps.yml build

# ------------------------------------------------------------------- firewall

if command -v ufw >/dev/null 2>&1; then
	log "configuring ufw: 22, 80, 443 only"
	ufw allow OpenSSH  >/dev/null || true
	ufw allow 'Nginx Full' >/dev/null 2>&1 || ufw allow 443/tcp >/dev/null || true
	ufw allow 80/tcp  >/dev/null || true
	ufw --force enable >/dev/null || true
	ufw status verbose || true
fi

# --------------------------------------------------------------------- start

log "starting stack"
docker compose --env-file "$ENV_FILE" -f deploy/compose.vps.yml up -d

log "waiting for the health check"
for _ in $(seq 1 30); do
	status="$(docker inspect --format '{{.State.Health.Status}}' \
		"$(docker compose -f docker-compose.yml -f deploy/compose.vps.yml ps -q app)" 2>/dev/null || echo starting)"
	[[ "$status" == "healthy" ]] && break
	sleep 5
done

log "---"
log "operator console:      https://$DOMAIN"
log "portal C2 endpoint:    https://$DOMAIN/c2/checkin"
log "headless C2 endpoint:  https://$DOMAIN/c2b/c2/checkin"
log ""
log "verify from outside:"
log "  curl -fsS https://$DOMAIN/healthz"
log "  curl -fsS -X POST --data-binary @frame https://$DOMAIN/c2b/c2/checkin -o /dev/null -w '%{http_code}\\n'"
log ""
log "Logs:  docker compose --env-file "$ENV_FILE" -f deploy/compose.vps.yml logs -f app c2"
log ""
log "Raw TCP/DNS listeners stay unpublished: they speak cleartext framed"
log "protocols. Enable them only behind a restricted lab network."
