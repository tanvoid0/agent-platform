"""Optional logical client_id scope for Process rows (namespace isolation)."""

from __future__ import annotations

import os

from fastapi import HTTPException

from models import Process


def require_client_id_enabled() -> bool:
    raw = (os.getenv("AGENT_PLATFORM_REQUIRE_CLIENT_ID") or "").strip().lower()
    return raw in ("1", "true", "yes", "on")


def merged_client_id(header_val: str | None, body_val: str | None) -> str | None:
    if header_val and header_val.strip():
        return header_val.strip()[:256]
    if body_val is not None and str(body_val).strip():
        return str(body_val).strip()[:256]
    return None


def assert_process_client_access(proc: Process | None, client_header: str | None) -> None:
    """If the process is scoped to a client_id, require a matching header or hide with 404."""
    if proc is None:
        return
    if proc.client_id is None:
        return
    eff = (client_header or "").strip()
    if eff != proc.client_id:
        raise HTTPException(status_code=404, detail="Process not found")
