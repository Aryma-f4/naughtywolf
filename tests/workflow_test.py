#!/usr/bin/env python3
"""Assert the CI workflow covers Linux and Windows verification.

Run: python3 tests/workflow_test.py
Exits non-zero when a required job or command is missing, so CI can gate
its own definition.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CI = ROOT / ".github/workflows/ci.yml"
DEPLOY = ROOT / ".github/workflows/deploy-vps.yml"

LINUX_JOB_NAME = "linux"
WINDOWS_JOB_NAME = "windows"

LINUX_COMMANDS = [
    "cargo fmt --check",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "cargo test --workspace",
    "node --test tests/*.cjs",
]

WINDOWS_COMMANDS = [
    "cargo test -p nw-profile -p nw-implant -p nw-console",
]

DEPLOY_VERIFY_COMMANDS = [
    "cargo fmt --check",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "cargo test --workspace",
]


def job_names(yaml_text):
    return re.findall(r"^\s{2}([a-zA-Z0-9_-]+):\s*$", yaml_text, re.MULTILINE)


def job_body(yaml_text, name):
    """Return the text of the job named `name` (naive indentation scan)."""
    lines = yaml_text.splitlines()
    start = None
    for index, line in enumerate(lines):
        if line.strip() == f"{name}:" and line.startswith("  "):
            start = index
            break
    if start is None:
        return None
    body = []
    for line in lines[start + 1:]:
        if not line or line.startswith(" ") or line.startswith("  #"):
            body.append(line)
        else:
            break
    return "\n".join(body) if body else None


def main():
    failures = []

    if not CI.exists():
        failures.append(f"{CI} does not exist")
    else:
        text = CI.read_text(encoding="utf-8")
        names = job_names(text)
        for required in (LINUX_JOB_NAME, WINDOWS_JOB_NAME):
            if required not in names:
                failures.append(f"ci.yml missing job {required!r}")
        for name, commands in (
            (LINUX_JOB_NAME, LINUX_COMMANDS),
            (WINDOWS_JOB_NAME, WINDOWS_COMMANDS),
        ):
            body = job_body(text, name)
            if body is None:
                continue
            for command in commands:
                if re.search(r"^\s*-?\s*run:\s*.*" + re.escape(command), body, re.MULTILINE) is None \
                        and command not in body:
                    failures.append(f"job {name!r} does not run {command!r}")
        linux = job_body(text, LINUX_JOB_NAME) or ""
        if "rust-cache" not in linux:
            failures.append("linux job does not cache Cargo by lockfile")

    if not DEPLOY.exists():
        failures.append(f"{DEPLOY} does not exist")
    else:
        deploy_text = DEPLOY.read_text(encoding="utf-8")
        if "needs: verify" not in deploy_text:
            failures.append("deploy job must depend on a verify job")
        verify = job_body(deploy_text, "verify")
        if verify is None:
            failures.append("deploy-vps.yml missing verify job")
        else:
            for command in DEPLOY_VERIFY_COMMANDS:
                if command not in verify:
                    failures.append(f"verify job does not run {command!r}")

    if failures:
        print("FAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("PASS: CI covers Linux, Windows, caching, and deployment gating")
    return 0


if __name__ == "__main__":
    sys.exit(main())