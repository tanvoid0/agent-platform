#!/usr/bin/env python3
"""Lightweight repository hygiene checks for CI/local use.

Checks:
1) No tracked paths contain backslashes.
2) No duplicate logical tracked paths after slash normalization.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def _tracked_paths() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files"],
        cwd=REPO_ROOT,
        check=True,
        text=True,
        capture_output=True,
    )
    return [line.strip() for line in out.stdout.splitlines() if line.strip()]


def check_git_paths() -> list[str]:
    errors: list[str] = []
    tracked = _tracked_paths()
    normalized_seen: dict[str, str] = {}
    for path in tracked:
        if "\\" in path:
            errors.append(f"Tracked path contains backslash: {path}")
        norm = path.replace("\\", "/").lower()
        prior = normalized_seen.get(norm)
        if prior and prior != path:
            errors.append(f"Duplicate logical path detected: {prior} <-> {path}")
        normalized_seen[norm] = path
    return errors


def main() -> int:
    failures = check_git_paths()
    if not failures:
        print("Hygiene checks passed.")
        return 0
    print("Hygiene checks failed:")
    for f in failures:
        print(f"- {f}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
