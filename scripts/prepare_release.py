#!/usr/bin/env python
"""Prepare a release: bump the version, sync the lockfile, cut the tag.

    python scripts/prepare_release.py patch
    python scripts/prepare_release.py minor
    python scripts/prepare_release.py 0.5.0
    python scripts/prepare_release.py current      # re-sync/tag what's already there

Flags:
    --dry-run   Print what would change; write nothing, commit nothing
    --no-tag    Bump and sync only — no commit, no tag, dirty tree left for you

The two crates are versioned in lockstep because `dist` derives the tag from
the version (`dist-workspace.toml` says why), so this writes both or neither.

Cargo.lock is the part that is easy to forget and expensive to get wrong:
`--locked` builds refuse to update it, so a manifest bumped without the lock
fails the release's smoke job, which gates every platform build — the tag then
publishes nothing. portal-desktop lost v0.7.0 to exactly that.

**The bump is committed before the tag is cut**, which is the whole reason this
commits at all: a tag made first names the commit *before* the bump, so the
release builds a tree still carrying the old version and `dist` plans a tag that
does not match it. Pushing is yours:

    git push && git push --tags
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DESKTOP = REPO / "desktop"
LOCK = DESKTOP / "Cargo.lock"

# Package name -> manifest. Both move together; see the module docstring.
CRATES = {
    "agent-platform-desktop": DESKTOP / "crates" / "app" / "Cargo.toml",
    "agent-platform-server": DESKTOP / "crates" / "server" / "Cargo.toml",
}

VERSION_RE = re.compile(r'^version = "([^"]+)"', re.MULTILINE)
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")


def run(cmd: list[str], **kw) -> str:
    print(f"[release] {' '.join(cmd)}")
    return subprocess.run(
        cmd, cwd=DESKTOP, check=True, capture_output=True, text=True, **kw
    ).stdout.strip()


def read_version(manifest: Path) -> str:
    match = VERSION_RE.search(manifest.read_text(encoding="utf-8"))
    if not match:
        sys.exit(f"No package version found in {manifest}")
    return match.group(1)


def resolve(spec: str, current: str) -> str:
    """`patch`/`minor`/`major`/`current`, or a literal version."""
    if spec == "current":
        return current
    if spec in ("patch", "minor", "major"):
        major, minor, patch = (int(p) for p in current.split("-")[0].split("."))
        if spec == "major":
            return f"{major + 1}.0.0"
        if spec == "minor":
            return f"{major}.{minor + 1}.0"
        return f"{major}.{minor}.{patch + 1}"
    if not SEMVER_RE.match(spec):
        sys.exit(f"Not a version or bump keyword: {spec}")
    return spec


def write_version(manifest: Path, version: str) -> None:
    text = manifest.read_text(encoding="utf-8")
    manifest.write_text(
        VERSION_RE.sub(f'version = "{version}"', text, count=1), encoding="utf-8"
    )


def sync_lock(version: str) -> None:
    """Refresh the workspace members' entries in Cargo.lock, then prove it.

    `--offline` because a version bump on a local member needs no registry, and
    a release prep that fails on a flaky network is a release prep that gets
    skipped.
    """
    run(["cargo", "update", "--workspace", "--offline"])
    lock = LOCK.read_text(encoding="utf-8")
    for name in CRATES:
        entry = re.search(rf'name = "{re.escape(name)}"\nversion = "([^"]+)"', lock)
        found = entry.group(1) if entry else None
        if found != version:
            sys.exit(
                f"Cargo.lock still reports {name} {found}; expected {version}. "
                "A --locked build (release smoke, then every platform job) would fail."
            )
    print(f"[release] Cargo.lock agrees on {version}")


def tag_exists(tag: str) -> bool:
    return (
        subprocess.run(
            ["git", "rev-parse", "-q", "--verify", f"refs/tags/{tag}"],
            cwd=REPO,
            capture_output=True,
        ).returncode
        == 0
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("spec", help="patch | minor | major | X.Y.Z | current")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--no-tag", action="store_true")
    args = parser.parse_args()

    versions = {name: read_version(path) for name, path in CRATES.items()}
    if len(set(versions.values())) != 1:
        print(f"[release] crates are out of lockstep: {versions} — realigning")
    current = max(versions.values())
    version = resolve(args.spec, current)
    tag = f"v{version}"

    if tag_exists(tag):
        sys.exit(f"Tag {tag} already exists — a spent version cannot be re-released.")

    print(f"[release] {current} -> {version} ({tag})")
    if args.dry_run:
        for name, path in CRATES.items():
            print(f"[dry-run] would write {version} to {path.relative_to(REPO)}")
        print(f"[dry-run] would sync Cargo.lock and tag {tag}")
        return 0

    for path in CRATES.values():
        write_version(path, version)
    sync_lock(version)

    if args.no_tag:
        print("[release] versions and lockfile written; commit and tag are yours")
        return 0

    paths = [str(p.relative_to(REPO)) for p in CRATES.values()] + ["desktop/Cargo.lock"]
    subprocess.run(["git", "add", *paths], cwd=REPO, check=True)
    subprocess.run(["git", "commit", "-m", f"chore: release {tag}"], cwd=REPO, check=True)
    subprocess.run(["git", "tag", "-a", tag, "-m", tag], cwd=REPO, check=True)
    print(f"[release] committed and tagged {tag} — `git push && git push --tags` releases it")
    return 0


if __name__ == "__main__":
    sys.exit(main())
