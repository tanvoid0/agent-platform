"""One request the desktop Status page can render itself from.

`/health` says the process answers, `/ready` says the database and workspace root are usable, and
`/api/v1/llm-proxy/...` says whether a provider is configured — but a status screen that fans out
to three endpoints shows three different moments and flickers between them. This returns one
snapshot instead.
"""

from __future__ import annotations

import os
import platform
import sys
import time
from typing import Any

from fastapi import APIRouter, Depends
from sqlmodel import Session, func, select

import database
from database import get_session
from health_checks import app_readiness_payload, llm_proxy_readiness_payload
from models import Process
from observability import log_ring
from workspace_service import workspace_root

router = APIRouter(prefix="/system", tags=["system"])

# Process start, so uptime is the server's own, not the host's.
_STARTED_AT = time.time()

# Runs a status page should surface as "there is work in flight". `approval_required` and
# `task_review_required` are included deliberately: they are stalled on a human, which is exactly
# the thing a monitoring screen exists to make visible.
ACTIVE_STATUSES = (
    "pending",
    "planning",
    "approval_required",
    "approved",
    "task_review_required",
    "running",
)


def _process_counts(session: Session) -> dict[str, int]:
    rows = session.exec(select(Process.status, func.count()).group_by(Process.status)).all()
    return {str(status): int(count) for status, count in rows}


def _paths() -> dict[str, str | None]:
    try:
        workspaces = str(workspace_root())
    except Exception:
        workspaces = None
    db_path = database.engine.url.database
    return {
        "database": str(db_path) if db_path else None,
        "database_backend": database.engine.url.get_backend_name(),
        "workspaces": workspaces,
        "llm_config_dir": os.getenv("CONFIG_DIR") or None,
        "model_ops_data": os.getenv("MODEL_OPS_DATA_DIR") or None,
    }


@router.get("/status")
def system_status(session: Session = Depends(get_session)) -> dict[str, Any]:
    app_code, app_ready = app_readiness_payload()
    proxy_code, proxy_ready = llm_proxy_readiness_payload()
    counts = _process_counts(session)
    return {
        "service": "agent-platform",
        "env": (os.getenv("AGENT_PLATFORM_ENV") or "development").strip().lower(),
        "uptime_seconds": round(time.time() - _STARTED_AT, 1),
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "listening_on": {
            "host": os.getenv("AGENT_PLATFORM_HOST") or "127.0.0.1",
            "port": int(os.getenv("AGENT_PLATFORM_PORT") or 18410),
        },
        "auth_required": bool((os.getenv("AGENT_PLATFORM_MASTER_KEY") or "").strip()),
        "readiness": {"ok": app_code == 200, **app_ready},
        "llm_proxy": {"ok": proxy_code == 200, **proxy_ready},
        "processes": {
            "by_status": counts,
            "active": sum(counts.get(s, 0) for s in ACTIVE_STATUSES),
            "total": sum(counts.values()),
        },
        "paths": _paths(),
    }


@router.get("/logs")
def system_logs(after: int = 0) -> dict[str, Any]:
    """Recent server log lines, one JSON object per line, newest last.

    Poll with the `next` from the previous response to get only what has been written since. The
    desktop shell has its own copy of this taken from the process's stdout — it covers startup and
    crashes, which this cannot, because this only answers while the server is running.
    """
    ring = log_ring()
    if ring is None:
        return {"lines": [], "next": after, "dropped": 0}
    return ring.snapshot(after)
