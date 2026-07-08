#!/usr/bin/env python3
"""Smoke checks for unified local dev and Docker workflows.

Runs offline checks (hygiene + fast API tests). Optionally probes a running server.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


def _run(cmd: list[str], *, cwd: Path | None = None) -> int:
    print(f"+ {' '.join(cmd)}")
    return subprocess.run(cmd, cwd=cwd or REPO_ROOT, check=False).returncode


def run_hygiene() -> int:
    return _run([sys.executable, "scripts/check_repo_hygiene.py"])


def run_api_smoke_tests() -> int:
    return _run(
        [
            sys.executable,
            "-m",
            "pytest",
            "-m",
            "contract",
            "-q",
            "--tb=short",
        ],
        cwd=REPO_ROOT / "app",
    )


def probe_health(base_url: str) -> int:
    url = f"{base_url.rstrip('/')}/health"
    try:
        with urllib.request.urlopen(url, timeout=8) as resp:
            body = resp.read().decode()
    except urllib.error.URLError as exc:
        print(f"FAIL: could not reach {url}: {exc}")
        return 1
    if resp.status != 200 or '"ok"' not in body:
        print(f"FAIL: unexpected response from {url}: status={resp.status} body={body[:200]!r}")
        return 1
    print(f"OK: {url}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Smoke-check agent-platform dev workflows.")
    parser.add_argument(
        "--live",
        metavar="URL",
        help="Probe GET /health on a running server (e.g. http://127.0.0.1:18410)",
    )
    parser.add_argument("--skip-tests", action="store_true", help="Hygiene only.")
    args = parser.parse_args()

    if run_hygiene() != 0:
        return 1
    if not args.skip_tests and run_api_smoke_tests() != 0:
        return 1
    if args.live and probe_health(args.live) != 0:
        return 1

    print("Smoke checks passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
