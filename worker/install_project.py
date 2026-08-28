"""Copy a bundled project scaffold into MODEL_OPS_DATA_DIR so a build can run.

    cd worker && PYTHONPATH=. python install_project.py jobhunt-screener

`ensure_data_scaffold` only ever copies `_template`, by design — a shipped
install should not grow somebody else's projects. This is the manual half of
that: name a scaffold under `model_ops/data/projects/` and it lands in the live
data dir, where `load_project` can find it.

`MODEL_OPS_DATA_DIR` and `CONFIG_DIR` decide where that is, exactly as they do
for a stage subprocess. With neither set the fallback is the repo's own
`data/llm`, not `./data/llm`, so the answer does not depend on which directory
this was run from.

The server adopts what it finds on disk (`sync_project_row`), but only when
something asks for the project by name, so the last step here pokes it: a
project the app cannot list is a project you assume did not install, and a 404
from a *running* server says it is reading a different data dir than this.
"""

from __future__ import annotations

import os
import shutil
import sys
import urllib.error
import urllib.request
from pathlib import Path

WORKER = Path(__file__).resolve().parent
os.environ.setdefault("CONFIG_DIR", str(WORKER.parent / "data" / "llm"))

sys.path.insert(0, str(WORKER))
from model_ops.paths import ensure_data_scaffold, projects_dir  # noqa: E402

BUNDLED = WORKER / "model_ops" / "data" / "projects"
BASE_URL = os.environ.get("AGENT_PLATFORM_BASE_URL", "http://127.0.0.1:18410").rstrip("/")
# A desktop install runs open on loopback (ADR 0013) and needs neither; a keyed
# one answers 401 without it, which is the same two env vars every other script
# here reads.
TOKEN = os.environ.get("AGENT_PLATFORM_MASTER_KEY") or os.environ.get("AGENT_PLATFORM_TOKEN")


def register(name: str) -> str:
    """Ask the server for the project, which is what writes its row."""
    request = urllib.request.Request(f"{BASE_URL}/api/v1/model-ops/projects/{name}")
    if TOKEN:
        request.add_header("Authorization", f"Bearer {TOKEN}")
    try:
        with urllib.request.urlopen(request, timeout=5):
            return "registered with the running server"
    except urllib.error.HTTPError as e:
        # Its message names the directory it looked in, which is the whole
        # diagnosis when it disagrees with this script's.
        return f"server answered {e.code}: {e.read().decode(errors='replace')[:300]}"
    except (urllib.error.URLError, OSError):
        return "server not reachable; it will pick this up on the first request for it"


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: python install_project.py <scaffold-name>", file=sys.stderr)
        return 2
    name = sys.argv[1]

    source = BUNDLED / name
    if not source.is_dir():
        available = sorted(p.name for p in BUNDLED.iterdir() if p.is_dir())
        print(f"No bundled scaffold {name!r}. Have: {', '.join(available)}", file=sys.stderr)
        return 1

    ensure_data_scaffold()
    dest = projects_dir() / name
    if dest.exists():
        # Never merge: the live copy owns knowledge/, datasets/ and adapters/,
        # and a half-overwrite is the kind of thing you find out about an hour
        # into a train run. Registering again is safe and is the whole reason
        # to re-run this once the server is up, so the second run is not an
        # error.
        print(f"already installed: {dest}")
    else:
        shutil.copytree(source, dest)
        (dest / "knowledge").mkdir(exist_ok=True)
        print(f"installed {name} -> {dest}")
    print(register(name))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
