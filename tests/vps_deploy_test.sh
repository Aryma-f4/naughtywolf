#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
deploy_script="$repo_root/deploy/vps/deploy.sh"

test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT

if "$deploy_script" invalid-sha >"$test_dir/stdout" 2>"$test_dir/stderr"; then
  echo "deploy script accepted a malformed commit SHA" >&2
  exit 1
fi

if ! grep -q "40-character hexadecimal commit SHA" "$test_dir/stderr"; then
  echo "deploy script did not explain the malformed SHA" >&2
  sed -n '1,20p' "$test_dir/stderr" >&2
  exit 1
fi

echo "vps deploy validation test passed"
