#!/usr/bin/env python3
"""
Copy MIT-licensed raster assets from the pixel-agents repo into web/public/pixel-agents/.

Upstream layout (webview-ui): https://github.com/pablodelucca/pixel-agents
Our public layout matches NOTICE.txt under web/public/pixel-agents/.

Usage (from agent-platform repo root):
  python scripts/sync_pixel_agents_assets.py
  python scripts/sync_pixel_agents_assets.py --source D:\\path\\to\\pixel-agents

Without --source: downloads the main-branch zip from GitHub and extracts the needed paths.
"""

from __future__ import annotations

import argparse
import shutil
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

ZIP_URL = "https://github.com/pablodelucca/pixel-agents/archive/refs/heads/main.zip"


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def _dest_root() -> Path:
    return _repo_root() / "web" / "public" / "pixel-agents"


def _copy_tree(src: Path, dst: Path) -> None:
    if not src.is_dir():
        raise FileNotFoundError(f"Missing source directory: {src}")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(src, dst, dirs_exist_ok=True)


def sync_from_clone(pa_root: Path) -> None:
    assets = pa_root / "webview-ui" / "public" / "assets"
    if not assets.is_dir():
        raise FileNotFoundError(f"Expected {assets} (clone pixel-agents with webview-ui assets)")

    dest = _dest_root()
    _copy_tree(assets / "characters", dest / "characters")
    _copy_tree(assets / "floors", dest / "assets" / "floors")
    _copy_tree(assets / "walls", dest / "assets" / "walls")
    _copy_tree(assets / "furniture", dest / "assets" / "furniture")


def sync_from_zip() -> None:
    with tempfile.TemporaryDirectory() as td:
        zpath = Path(td) / "pixel-agents.zip"
        print(f"Downloading {ZIP_URL} …", file=sys.stderr)
        urllib.request.urlretrieve(ZIP_URL, zpath)
        with zipfile.ZipFile(zpath, "r") as zf:
            zf.extractall(Path(td))
        # GitHub zip contains a single top folder: pixel-agents-main
        roots = [p for p in Path(td).iterdir() if p.is_dir() and p.name.startswith("pixel-agents")]
        if len(roots) != 1:
            raise RuntimeError(f"Expected one pixel-agents-* folder in zip, got {[p.name for p in Path(td).iterdir()]}")
        sync_from_clone(roots[0])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        type=Path,
        help="Path to a local pixel-agents repository root (uses webview-ui/public/assets).",
    )
    args = parser.parse_args()

    dest = _dest_root()
    if not dest.parent.is_dir():
        print(f"Refusing to run: missing {dest.parent}", file=sys.stderr)
        return 2

    if args.source:
        sync_from_clone(args.source.resolve())
    else:
        sync_from_zip()

    print(f"Synced raster assets into {dest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
