"""Optional Bearer auth for agent-platform HTTP APIs (single shared secret)."""

from __future__ import annotations

import os
import secrets

from fastapi import Header, HTTPException


def agent_platform_api_key_expected() -> str | None:
    v = (os.getenv("AGENT_PLATFORM_API_KEY") or "").strip()
    return v or None


async def verify_agent_platform_api_key(
    authorization: str | None = Header(None),
) -> None:
    expected = agent_platform_api_key_expected()
    if not expected:
        return
    if not authorization or not authorization.startswith("Bearer "):
        raise HTTPException(
            status_code=401,
            detail="Missing or invalid Authorization (expected Bearer token)",
        )
    token = authorization[7:].strip()
    if not secrets.compare_digest(token, expected):
        raise HTTPException(status_code=401, detail="Invalid API key")


async def agent_platform_client_header(
    x_agent_platform_client: str | None = Header(None, alias="X-Agent-Platform-Client"),
) -> str | None:
    if x_agent_platform_client is None:
        return None
    s = x_agent_platform_client.strip()
    if not s:
        return None
    return s[:256]
