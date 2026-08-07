#!/usr/bin/env python
"""Build the Windows installer end to end.

    python scripts/build_installer.py

Orchestrates:
    cargo build --release -p agent-platform-desktop   (desktop/target/release/agent-platform.exe)
    iscc desktop/installer/agent-platform.iss           (dist/agent-platform-setup.exe)

There is no bundling step any more. It used to run scripts/bundle_server.py to
assemble an embedded CPython plus the whole app/ package into desktop/payload/;
the server is Rust now, and the only Python that ships is worker/, which the
.iss copies straight from the checkout.

Windows only — this app has only ever been built and run on Windows; macOS/Linux
packaging (and their own signing/notarization) is not implemented.

Cargo features come from AGENT_PLATFORM_FEATURES (comma-separated, e.g. "cuda"),
which is how a build gets in-process inference — see ADR 0006. That build links
llama.cpp as DLLs, which the installer picks up beside the exe.

Signing is optional and off by default. Set AGENT_PLATFORM_SIGN_CERT to a .pfx
path (and AGENT_PLATFORM_SIGN_PASSWORD, AGENT_PLATFORM_SIGN_TIMESTAMP_URL
optionally) to sign agent-platform.exe before it's packaged. Without a real
code-signing certificate the build stays unsigned — see docs/native-desktop-migration.md
item 5 for why a self-signed cert isn't used here.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DESKTOP = REPO / "desktop"
EXE = DESKTOP / "target" / "release" / "agent-platform.exe"
ISS = DESKTOP / "installer" / "agent-platform.iss"


def _run(cmd: list[str], **kw) -> None:
    print(f"[installer] {' '.join(str(c) for c in cmd)}")
    subprocess.run(cmd, check=True, **kw)


def sign_exe() -> None:
    """Sign agent-platform.exe if the user has pointed us at their own cert.

    ponytail: no self-signed fallback — an unsigned build is honest, a
    self-signed one is misleading. Real signing needs the user's own cert.
    """
    cert = os.environ.get("AGENT_PLATFORM_SIGN_CERT")
    if not cert:
        print("[installer] AGENT_PLATFORM_SIGN_CERT not set; shipping unsigned build")
        return
    signtool = shutil.which("signtool")
    if not signtool:
        sys.exit("AGENT_PLATFORM_SIGN_CERT is set but signtool.exe is not on PATH "
                  "(install the Windows SDK).")
    password = os.environ.get("AGENT_PLATFORM_SIGN_PASSWORD", "")
    timestamp_url = os.environ.get(
        "AGENT_PLATFORM_SIGN_TIMESTAMP_URL", "http://timestamp.digicert.com"
    )
    cmd = [signtool, "sign", "/f", cert]
    if password:
        cmd += ["/p", password]
    cmd += ["/t", timestamp_url, str(EXE)]
    _run(cmd)


def check_local_llm_dlls(features: str) -> None:
    """A `local-llm` build is useless without llama.cpp's DLLs beside the exe.

    The feature forces `dynamic-link` (two static ggmls will not link), so cargo
    drops the DLLs in target\\release and the .iss picks them up with a wildcard.
    A wildcard that quietly matches nothing is exactly how a broken installer
    ships, hence this check rather than trusting the glob.
    """
    if not any(f in features.split(",") for f in ("local-llm", "cuda")):
        return
    found = {p.name for p in EXE.parent.glob("*.dll")}
    missing = {"llama.dll", "ggml.dll", "ggml-base.dll"} - found
    if missing:
        sys.exit(
            f"AGENT_PLATFORM_FEATURES={features} but {', '.join(sorted(missing))} "
            f"is not in {EXE.parent} — the installed app would die at startup."
        )
    print(f"[installer] bundling {len(found)} llama.cpp/ggml DLL(s)")
    if "cuda" in features.split(",") and not any(n.startswith("cudart64") for n in found):
        # cuBLAS alone is several hundred MB, so the redistributables are not
        # bundled: a CUDA build targets machines that already have the toolkit.
        print(
            "[installer] warning: CUDA build without the CUDA runtime DLLs — "
            "target machines need the CUDA Toolkit on PATH or the app exits with 0xC0000135"
        )


def main() -> None:
    if sys.platform != "win32":
        sys.exit(
            "build_installer.py only supports Windows — this app has only ever "
            "been built and run there. macOS/Linux packaging is not implemented."
        )

    # Inno Setup's installer never adds itself to PATH, so also try its
    # default per-user and per-machine install dirs.
    candidates = [
        Path(os.environ.get("LOCALAPPDATA", "")) / "Programs" / "Inno Setup 6" / "ISCC.exe",
        Path("C:/Program Files (x86)/Inno Setup 6/ISCC.exe"),
    ]
    iscc = shutil.which("iscc") or next(
        (str(p) for p in candidates if p.is_file()), None
    )
    if not iscc:
        sys.exit(
            "iscc (Inno Setup Compiler) not found on PATH or in its default "
            "install locations.\n"
            "Install Inno Setup from https://jrsoftware.org/isinfo.php and re-run."
        )

    # The app spawns agent-platformd (ADR 0007) from its own directory, so both
    # binaries have to be built and both have to be packaged.
    build = ["cargo", "build", "--release", "-p", "agent-platform-desktop", "-p", "agent-platform-server"]
    features = os.environ.get("AGENT_PLATFORM_FEATURES", "").strip()
    if features:
        build += ["--features", features]
    _run(build, cwd=DESKTOP)
    check_local_llm_dlls(features)
    sign_exe()
    _run([iscc, str(ISS)])

    out = REPO / "dist" / "agent-platform-setup.exe"
    if out.is_file():
        print(f"[installer] built {out} ({out.stat().st_size / 1_048_576:.1f} MiB)")
    else:
        sys.exit(f"iscc reported success but {out} is missing")


if __name__ == "__main__":
    main()
