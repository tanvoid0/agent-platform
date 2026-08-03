#!/usr/bin/env python
"""Stage a self-contained copy of the server for the desktop shell to bundle.

    python scripts/bundle_server.py            # build desktop/payload/
    python scripts/bundle_server.py --clean    # rebuild it from scratch
    python scripts/bundle_server.py --selfcheck

Produces:

    desktop/payload/
      runtime/     relocatable CPython with app/requirements.txt installed
      app/         the server source, minus caches, tests and dev databases
      config/      agent_platform.yaml, the non-secret defaults
      scripts/     start.py, the sidecar entrypoint

Uses uv's managed CPython (python-build-standalone), which is relocatable — a plain `venv`
is not, because it points back at a base interpreter the user's machine will not have.

Torch is deliberately absent: GPU stages run out-of-process (see MODEL_OPS_PYTHON in
app/model_ops/runner.py), so the shipped server stays small and the training environment is
installed on demand.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PAYLOAD = REPO / "desktop" / "payload"
REQUIREMENTS = REPO / "app" / "requirements.txt"
PYTHON_VERSION = "3.12"

# Caches, test-only code, and the developer's own databases must never reach a user's machine.
EXCLUDE_NAMES = {"__pycache__", ".pytest_cache", "tests", "data", ".mypy_cache", ".ruff_cache"}
EXCLUDE_SUFFIXES = (".db", ".db-wal", ".db-shm", ".pyc", ".lock")


def ignored(_dir: str, names: list[str]) -> set[str]:
    """shutil.copytree filter: drop caches, tests and local databases."""
    return {
        n
        for n in names
        if n in EXCLUDE_NAMES or n.endswith(EXCLUDE_SUFFIXES) or ".db." in n
    }


def _run(cmd: list[str], check: bool = True, **kw) -> subprocess.CompletedProcess:
    print(f"[bundle] {' '.join(str(c) for c in cmd)}")
    return subprocess.run(cmd, check=check, **kw)


def ensure_uv() -> str:
    uv = shutil.which("uv")
    if uv:
        return uv
    print("[bundle] uv not found; installing it")
    _run([sys.executable, "-m", "pip", "install", "--quiet", "uv"])
    uv = shutil.which("uv")
    if not uv:
        # pip may have put it in a Scripts/bin dir that is not on PATH.
        return f"{sys.executable} -m uv"
    return uv


def _uv(uv: str) -> list[str]:
    return uv.split() if " " in uv else [uv]


def _uv_out(uv: str, *args: str) -> str:
    return subprocess.run(
        _uv(uv) + list(args), check=True, capture_output=True, text=True
    ).stdout.strip()


def managed_python(uv: str) -> Path:
    """Path to uv's managed CPython, installing it first if needed.

    The install step is advisory — it reports a link error when the runtime is already there —
    so the lookup below is the real gate.
    """
    _run(_uv(uv) + ["python", "install", PYTHON_VERSION], check=False)
    root = Path(_uv_out(uv, "python", "dir"))
    found = _uv_out(uv, "python", "find", PYTHON_VERSION)
    if not found:
        sys.exit("uv could not report the path of its managed Python.")
    found_path = Path(found)
    # `python find` happily returns a system interpreter of the right version. Bundling one would
    # produce an installer that works here and breaks on every machine without that interpreter.
    if root not in found_path.parents:
        sys.exit(
            f"uv resolved {found_path}, which is not a managed runtime under {root}.\n"
            f"Run: uv python install {PYTHON_VERSION}"
        )
    return found_path


def runtime_root(python_exe: Path) -> Path:
    """The relocatable distribution root: python.exe sits at the top on Windows, in bin/ elsewhere."""
    return python_exe.parent if python_exe.parent.name.lower() != "bin" else python_exe.parent.parent


def payload_python(root: Path) -> Path:
    return root / "python.exe" if sys.platform == "win32" else root / "bin" / "python3"


def main() -> None:
    args = set(sys.argv[1:])
    if "--selfcheck" in args:
        return _selfcheck()

    if "--clean" in args and PAYLOAD.exists():
        shutil.rmtree(PAYLOAD)
    PAYLOAD.mkdir(parents=True, exist_ok=True)

    uv = ensure_uv()
    source_runtime = runtime_root(managed_python(uv))
    target_runtime = PAYLOAD / "runtime"
    if not target_runtime.exists():
        print(f"[bundle] copying {source_runtime} -> {target_runtime}")
        shutil.copytree(source_runtime, target_runtime, ignore=ignored)

    # uv marks its own runtimes EXTERNALLY-MANAGED and refuses to install into them. This copy is
    # no longer uv's — it is the payload we are about to install the server's dependencies into.
    for marker in target_runtime.rglob("EXTERNALLY-MANAGED"):
        print(f"[bundle] dropping {marker.relative_to(target_runtime)} from the copied runtime")
        marker.unlink()

    _run(
        _uv(uv)
        + ["pip", "install", "--python", str(payload_python(target_runtime)), "-r", str(REQUIREMENTS)]
    )

    # config/ sits beside app/ because that is where platform_config looks for agent_platform.yaml.
    for source, target in (
        (REPO / "app", PAYLOAD / "app"),
        (REPO / "config", PAYLOAD / "config"),
    ):
        if not source.is_dir():
            sys.exit(f"{source} is missing.")
        if target.exists():
            shutil.rmtree(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source, target, ignore=ignored)

    scripts = PAYLOAD / "scripts"
    scripts.mkdir(exist_ok=True)
    shutil.copy2(REPO / "scripts" / "start.py", scripts / "start.py")

    size_mb = sum(f.stat().st_size for f in PAYLOAD.rglob("*") if f.is_file()) / 1e6
    print(f"[bundle] payload ready at {PAYLOAD} ({size_mb:.0f} MB)")


def _selfcheck() -> None:
    names = [
        "main.py",
        "__pycache__",
        "tests",
        "data",
        "agent_runs.db",
        "agent_platform.db.startup.lock",
        "models.pyc",
        "requirements.txt",
    ]
    dropped = ignored("app", names)
    assert dropped == {
        "__pycache__",
        "tests",
        "data",
        "agent_runs.db",
        "agent_platform.db.startup.lock",
        "models.pyc",
    }, dropped
    assert "main.py" not in dropped and "requirements.txt" not in dropped

    win = Path("C:/py/python.exe")
    nix = Path("/opt/py/bin/python3")
    assert runtime_root(win) == Path("C:/py"), runtime_root(win)
    assert runtime_root(nix) == Path("/opt/py"), runtime_root(nix)
    print("selfcheck ok")


if __name__ == "__main__":
    main()
