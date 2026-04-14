"""Call the Agent Platform HTTP API from another backend (no Flow UI).

Demonstrates: resolve a team template id, start a process, poll until a terminal
state, print task outputs.

Environment:
  AGENT_PLATFORM_BASE_URL   Base URL (default http://127.0.0.1:18410)
  AGENT_PLATFORM_MASTER_KEY Required when the server enforces auth
  TEAM_TEMPLATE_ID          Optional numeric id (skips name lookup)
  TEAM_TEMPLATE_NAME        Pick template by name (default: CV Reviewer Agency)

Example:
  set AGENT_PLATFORM_MASTER_KEY=your-key
  python scripts/external_microservice_example.py
"""

from __future__ import annotations

import json
import os
import sys
import time

try:
    import httpx
except ImportError:
    print("Install dependencies: pip install -r app/requirements.txt", file=sys.stderr)
    raise

DEFAULT_BASE = "http://127.0.0.1:18410"
DEFAULT_TEAM_NAME = "CV Reviewer Agency"
POLL_SECONDS = 1.5
MAX_WAIT_S = 600


def _headers() -> dict[str, str]:
    key = (os.getenv("AGENT_PLATFORM_MASTER_KEY") or "").strip()
    if not key:
        return {}
    return {"Authorization": f"Bearer {key}"}


def _resolve_team_template_id(client: httpx.Client, base: str) -> int:
    raw = (os.getenv("TEAM_TEMPLATE_ID") or "").strip()
    if raw.isdigit():
        return int(raw)
    name = (os.getenv("TEAM_TEMPLATE_NAME") or DEFAULT_TEAM_NAME).strip()
    r = client.get(f"{base}/api/v1/teams/", headers=_headers(), timeout=30.0)
    r.raise_for_status()
    data = r.json()
    teams = data.get("teams") or []
    for t in teams:
        if (t.get("name") or "").strip() == name:
            return int(t["id"])
    names = [t.get("name") for t in teams]
    raise SystemExit(
        f"No team template named {name!r}. Set TEAM_TEMPLATE_ID or TEAM_TEMPLATE_NAME. "
        f"Available: {names}"
    )


def main() -> None:
    base = (os.getenv("AGENT_PLATFORM_BASE_URL") or DEFAULT_BASE).rstrip("/")
    goal = (os.getenv("EXAMPLE_PROCESS_GOAL") or "").strip() or (
        "Briefly list 3 strengths and 2 improvements for this CV (fictional content): "
        "Jane Doe, software engineer, 5 years Python."
    )
    auto = (os.getenv("EXAMPLE_AUTO_APPROVE") or "true").strip().lower() in (
        "1",
        "true",
        "yes",
    )
    client_id = (os.getenv("EXAMPLE_CLIENT_ID") or "external-microservice-example").strip()

    with httpx.Client() as client:
        team_id = _resolve_team_template_id(client, base)
        body = {
            "goal": goal,
            "auto_approve": auto,
            "team_template_id": team_id,
            "client_id": client_id,
        }
        r = client.post(
            f"{base}/api/v1/processes",
            headers={**_headers(), "X-Agent-Platform-Client": client_id},
            json=body,
            timeout=60.0,
        )
        if r.status_code >= 400:
            print(r.text, file=sys.stderr)
        r.raise_for_status()
        start = r.json()
        pid = int(start["process_id"])
        print("Started process_id:", pid, "status:", start.get("status"))

        deadline = time.monotonic() + MAX_WAIT_S
        last_status = ""
        while time.monotonic() < deadline:
            pr = client.get(
                f"{base}/api/v1/processes/{pid}",
                headers={**_headers(), "X-Agent-Platform-Client": client_id},
                timeout=30.0,
            )
            pr.raise_for_status()
            payload = pr.json()
            proc = payload.get("process") or {}
            status = proc.get("status") or ""
            if status != last_status:
                print("status:", status)
                last_status = status

            if status in ("completed", "failed", "cancelled"):
                tasks = payload.get("tasks") or []
                print("\nTasks:")
                for t in sorted(tasks, key=lambda x: x.get("id") or 0):
                    tid = t.get("id")
                    role = t.get("role")
                    out = t.get("output")
                    st = t.get("status")
                    line = f"  [{tid}] {role} ({st})"
                    print(line)
                    if out:
                        preview = out if len(out) <= 500 else out[:500] + "…"
                        print("    output:", preview.replace("\n", "\n    "))
                if status != "completed" and proc.get("failure_reason"):
                    print("failure_reason:", proc.get("failure_reason"))
                return

            if status in ("approval_required", "task_review_required"):
                print(
                    "Stopped at gate:",
                    status,
                    "— approve DAG or review tasks via API, then re-run or extend this script.",
                    file=sys.stderr,
                )
                print(json.dumps(payload, indent=2, default=str))
                raise SystemExit(2)

            time.sleep(POLL_SECONDS)

        raise SystemExit("Timeout waiting for process to finish")


if __name__ == "__main__":
    main()
