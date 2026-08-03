#!/usr/bin/env python
"""Build the Windows installer end to end.

    python scripts/build_installer.py

Orchestrates:
    cargo build --release -p agent-platform-desktop   (desktop/target/release/agent-platform.exe)
    python scripts/bundle_server.py                    (desktop/payload/)
    iscc desktop/installer/agent-platform.iss           (dist/agent-platform-setup.exe)

Windows only — this app has only ever been built and run on Windows; macOS/Linux
packaging (and their own signing/notarization) is not implemented.

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


def main() -> None:
    if sys.platform != "win32":
        sys.exit(
            "build_installer.py only supports Windows — this app has only ever "
            "been built and run there. macOS/Linux packaging is not implemented."
        )

    iscc = shutil.which("iscc")
    if not iscc:
        sys.exit(
            "iscc (Inno Setup Compiler) not found on PATH.\n"
            "Install Inno Setup from https://jrsoftware.org/isinfo.php and re-run."
        )

    _run(["cargo", "build", "--release", "-p", "agent-platform-desktop"], cwd=DESKTOP)
    _run([sys.executable, str(REPO / "scripts" / "bundle_server.py")])
    sign_exe()
    _run([iscc, str(ISS)])

    out = REPO / "dist" / "agent-platform-setup.exe"
    if out.is_file():
        print(f"[installer] built {out} ({out.stat().st_size / 1_048_576:.1f} MiB)")
    else:
        sys.exit(f"iscc reported success but {out} is missing")


if __name__ == "__main__":
    main()
