"""Smoke-test a built portal image using a disposable, network-isolated container.

Run: python3 tests/container_smoke.py naughtywolf:coolify-check
Only this test's randomly named container and volume are removed on exit.
"""
import os
import secrets
import subprocess
import sys
import time
import uuid


def docker(*args, input=None, check=True):
    return subprocess.run(
        ["docker", *args], input=input, capture_output=True, text=True,
        check=check, timeout=120, env=environment,
    )


image = sys.argv[1] if len(sys.argv) > 1 else "naughtywolf:coolify-check"
name = "nw-smoke-" + uuid.uuid4().hex[:12]
volume = name + "-data"
environment = dict(os.environ, NAUGHTYWOLF_SESSION_SECRET=secrets.token_hex(32),
                   NAUGHTYWOLF_C2_PSK=secrets.token_hex(32))


def start():
    docker("run", "-d", "--name", name, "--network", "none",
           "--mount", f"type=volume,source={volume},target=/data",
           "-e", "NAUGHTYWOLF_SESSION_SECRET", "-e", "NAUGHTYWOLF_C2_PSK",
           "-e", "NAUGHTYWOLF_COOKIE_SECURE=false", image)
    for _ in range(60):
        status = docker("inspect", "--format", "{{.State.Health.Status}}", name).stdout.strip()
        if status == "healthy":
            return
        if docker("inspect", "--format", "{{.State.Running}}", name).stdout.strip() != "true":
            raise RuntimeError("Container exited before becoming healthy")
        time.sleep(1)
    raise RuntimeError("Container did not become healthy within 60 seconds")


try:
    docker("volume", "create", volume)
    start()
    assert docker("exec", name, "id", "-u").stdout.strip() == "10001"
    status = docker("exec", name, "curl", "-s", "-o", "/dev/null", "-w", "%{http_code}",
                    "http://127.0.0.1:8080/healthz").stdout
    assert status == "204", status
    login = docker("exec", name, "curl", "-fsS", "http://127.0.0.1:8080/login").stdout
    assert 'name="csrf_token"' in login and "/static/motion.js" in login
    for path in ["admin.css", "admin.js", "anime.min.js", "motion.js", "workspace.js", "workspace.css"]:
        assert docker("exec", name, "curl", "-fsS", f"http://127.0.0.1:8080/static/{path}").stdout
    docker("exec", "-i", name, "naughtywolf", "user", "create", "--username", "smoke-admin",
           "--role", "admin", "--password-stdin", input=secrets.token_urlsafe(24) + "\n")
    docker("exec", name, "sh", "-ec",
           "test -w /app/target && test -w /usr/local/cargo && test -w /usr/local/rustup; "
           "rustc --version; cargo metadata --manifest-path /app/Cargo.toml --no-deps --offline --format-version 1 >/dev/null; "
           "printf 'int main(void) { return 0; }' > /tmp/nw-cross-probe.c; "
           "musl-gcc /tmp/nw-cross-probe.c -o /tmp/nw-cross-probe-musl; "
           "x86_64-w64-mingw32-gcc /tmp/nw-cross-probe.c -o /tmp/nw-cross-probe.exe; "
           "x86_64-w64-mingw32-dlltool --version >/dev/null; "
           "printf evidence-persisted > /data/evidence/smoke.txt; printf payload-persisted > /data/payloads/smoke.txt")
    docker("stop", name)
    docker("rm", name)
    start()
    assert "smoke-admin" in docker("exec", name, "naughtywolf", "user", "list").stdout
    assert docker("exec", name, "cat", "/data/evidence/smoke.txt").stdout == "evidence-persisted"
    assert docker("exec", name, "cat", "/data/payloads/smoke.txt").stdout == "payload-persisted"
    print("PASS: health, UI assets, non-root CLI, cross-build toolchains, and data persistence after replacement")
except Exception:
    print(docker("logs", "--tail", "40", name, check=False).stdout, file=sys.stderr)
    raise
finally:
    docker("rm", "-f", name, check=False)
    docker("volume", "rm", volume, check=False)
