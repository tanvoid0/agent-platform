"""Reusable dependency injection patterns for routes."""

from __future__ import annotations

from typing import NamedTuple

from fastapi import Depends
from sqlmodel import Session

from api_auth import agent_platform_client_header
from database import get_session


class SessionWithAuth(NamedTuple):
    """Container for session + optional auth client header."""

    session: Session
    client_hdr: str | None


async def session_with_auth(
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
) -> SessionWithAuth:
    """Dependency that provides both session and auth header in one call."""
    return SessionWithAuth(session=session, client_hdr=client_hdr)
