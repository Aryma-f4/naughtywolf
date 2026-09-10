#!/usr/bin/env bash
set -Eeuo pipefail

commit_sha=${1:-}
if [[ ! "$commit_sha" =~ ^[0-9a-fA-F]{40}$ ]]; then
  echo "usage: $0 <40-character hexadecimal commit SHA>" >&2
  exit 64
fi

deploy_dir=${NAUGHTYWOLF_DEPLOY_DIR:-/opt/naughtywolf}
domain=${NAUGHTYWOLF_DOMAIN:-gateofbabylon.space}

exec 9>/tmp/naughtywolf-deploy.lock
if ! flock -n 9; then
  echo "another NaughtyWolf deployment is already running" >&2
  exit 75
fi

cd "$deploy_dir"

if [[ ! -f .env ]]; then
  echo "$deploy_dir/.env is missing" >&2
  exit 78
fi

git fetch --prune origin develop
if ! git cat-file -e "${commit_sha}^{commit}" 2>/dev/null; then
  echo "commit $commit_sha was not fetched from origin" >&2
  exit 65
fi
if ! git merge-base --is-ancestor "$commit_sha" origin/develop; then
  echo "commit $commit_sha is not part of origin/develop" >&2
  exit 65
fi

git checkout --force --detach "$commit_sha"

compose=(
  sudo docker compose
  --project-name naughtywolf
  --env-file .env
  --file docker-compose.yml
  --file deploy/vps/docker-compose.yml
)

"${compose[@]}" config --quiet
"${compose[@]}" up --build --detach --remove-orphans

container_id=$("${compose[@]}" ps --quiet app)
if [[ -z "$container_id" ]]; then
  echo "NaughtyWolf application container was not created" >&2
  exit 1
fi

healthy=false
for _ in $(seq 1 60); do
  state=$(sudo docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container_id")
  if [[ "$state" == healthy ]]; then
    healthy=true
    break
  fi
  if [[ "$state" == exited || "$state" == dead ]]; then
    break
  fi
  sleep 2
done

if [[ "$healthy" != true ]]; then
  "${compose[@]}" ps >&2
  "${compose[@]}" logs --tail 120 app >&2
  echo "NaughtyWolf application did not become healthy" >&2
  exit 1
fi

public_healthy=false
for _ in $(seq 1 360); do
  status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
    --max-time 10 "https://${domain}/healthz" || true)
  if [[ "$status" == 204 ]]; then
    public_healthy=true
    break
  fi
  sleep 2
done

if [[ "$public_healthy" != true ]]; then
  echo "https://${domain}/healthz did not return HTTP 204" >&2
  exit 1
fi

echo "NaughtyWolf $commit_sha is healthy at https://${domain}"
