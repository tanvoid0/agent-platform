#!/usr/bin/env python
"""Start agent-platform's FastAPI server as a single local process.

    python scripts/start.py                    # serve, open browser
    python scripts/start.py --no-browser
    python scripts/start.py --exit-with-parent # die when the process that spawned us does

The desktop shell (desktop/) runs this as its sidecar with --exit-with-parent.
"""

from __future__ import annotations

import os
import threading
import webbrowser
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def exit_when_parent_dies() -> None:
    """Exit once the parent's stdin pipe hits EOF.

    The parent holds the write end, so EOF means it is gone — including when it was killed
    rather than shut down, which a PID check would miss.

    fd 0 is then pointed at devnull. Every subprocess the server starts (Alembic migrations,
    GPU training stages, the coder sandbox) inherits fd 0, and on Windows a child holding the
    parent's pipe blocks — Alembic hung for its full 120s timeout before this. Reading the
    dup'd fd directly also keeps the watchdog off ``sys.stdin``'s buffer lock, which otherwise
    triggers a fatal error when the interpreter finalizes while the thread is blocked.
    """
    pipe_fd = os.dup(0)
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)
    os.close(devnull)

    def _watch() -> None:
        try:
            while os.read(pipe_fd, 4096):
                pass
        except OSError:  # a closed or invalid handle means the parent is gone too
            pass
        os._exit(0)

    threading.Thread(target=_watch, daemon=True).start()


def main() -> None:
    import sys

    args = set(sys.argv[1:])
    if "--exit-with-parent" in args:
        exit_when_parent_dies()

    port = int(os.getenv("AGENT_PLATFORM_PORT", "18410"))
    host = os.getenv("AGENT_PLATFORM_HOST", "127.0.0.1")
    # No browser UI ships any more (the desktop app is the UI); the only page
    # worth opening is the API reference.
    url = f"http://{host}:{port}/docs"

    if "--no-browser" not in args:
        threading.Timer(1.5, webbrowser.open, [url]).start()

    print(f"[start] API      http://{host}:{port}/api/v1")
    print(f"[start] API docs {url}")
    if not (os.getenv("AGENT_PLATFORM_MASTER_KEY") or "").strip():
        print("[start] auth open (no AGENT_PLATFORM_MASTER_KEY set)")

    sys.path.insert(0, str(REPO / "app"))
    import uvicorn

    uvicorn.run("main:app", host=host, port=port, log_level="info")


if __name__ == "__main__":
    main()
