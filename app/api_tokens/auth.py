"""Unified Bearer auth: project API tokens (agp_...) or the master key, one dependency.

Replaces `verify_agent_platform_api_key` as the router-level auth dependency.
Master-key callers get an unrestricted TokenPrincipal (project_id=None) — same
as today. Project-token callers get a principal scoped to one project, which
route handlers use for project-access checks (see client_scope.py-style 404
isolation) and usage attribution.
"""

from __future__ import annotations

import secrets as _secrets
from datetime import datetime
from typing import NamedTuple

from fastapi import Depends, Header, Request
from sqlmodel import Session, select

from api_auth import agent_platform_api_key_expected
from api_tokens.exceptions import (
    InsufficientScopeError,
    TokenExpiredError,
    TokenHeldError,
    TokenNotFoundError,
    TokenRevokedError,
)
from api_tokens.rate_limiter import check_and_increment
from api_tokens.token_service import hash_token
from database import get_session
from models import ApiToken
from observability import update_request_context
from time_utils import utc_now_naive

_LAST_USED_THROTTLE_SECONDS = 60
_TOKEN_PREFIX_MARKER = "agp_"


class TokenPrincipal(NamedTuple):
    project_id: int | None  # legacy per-token project binding (unused for scoping)
    token_id: int | None
    scopes: list[str]
    workspace_id: int | None = None  # None => master-key / unrestricted caller


def verify_project_api_token(required_scope: str | None = None):
    async def _dependency(
        request: Request,
        authorization: str | None = Header(None),
        session: Session = Depends(get_session),
    ) -> TokenPrincipal:
        expected_master_key = agent_platform_api_key_expected()
        if not expected_master_key:
            # Dev convenience: no master key configured means auth is fully open
            # (preserves the pre-existing behavior of verify_agent_platform_api_key).
            return TokenPrincipal(project_id=None, token_id=None, scopes=["*"])

        if not authorization or not authorization.startswith("Bearer "):
            raise TokenNotFoundError("Missing or invalid Authorization (expected Bearer token)")
        raw = authorization[7:].strip()

        if raw.startswith(_TOKEN_PREFIX_MARKER):
            row = session.exec(
                select(ApiToken).where(ApiToken.token_hash == hash_token(raw))
            ).first()
            if row is None:
                raise TokenNotFoundError("Invalid API token")
            if row.status == "revoked":
                raise TokenRevokedError("This token has been revoked.", token_prefix=row.prefix)
            if row.status == "held":
                raise TokenHeldError(
                    row.held_reason or "This token is temporarily on hold.",
                    token_prefix=row.prefix,
                )
            if row.expires_at and row.expires_at < utc_now_naive():
                raise TokenExpiredError("This token has expired.", token_prefix=row.prefix)

            scopes = row.scopes
            if required_scope and required_scope not in scopes and "*" not in scopes:
                raise InsufficientScopeError(
                    f"Token lacks required scope '{required_scope}'.", token_prefix=row.prefix
                )

            check_and_increment(row.id, row.rate_limit_per_minute)

            now = utc_now_naive()
            if not row.last_used_at or (now - row.last_used_at).total_seconds() > _LAST_USED_THROTTLE_SECONDS:
                row.last_used_at = now
                session.add(row)
                session.commit()

            principal = TokenPrincipal(
                project_id=row.project_id,
                token_id=row.id,
                scopes=scopes,
                workspace_id=row.workspace_id,
            )
            request.state.workspace_id = row.workspace_id
            update_request_context(workspace_id=row.workspace_id)
            return principal

        if not _secrets.compare_digest(raw, expected_master_key):
            raise TokenNotFoundError("Invalid API key")
        request.state.workspace_id = None
        return TokenPrincipal(project_id=None, token_id=None, scopes=["*"], workspace_id=None)

    return _dependency


# Single module-level singleton so FastAPI's per-request dependency cache
# (keyed by callable identity) dedupes the router-level check with any
# route-level `Depends(require_valid_token)` instead of re-running the
# token lookup/rate-limit twice per request. Per-endpoint scope requirements
# are enforced separately via `require_scope` below (not via a second,
# differently-scoped dependency instance, which would bypass the cache).
require_valid_token = verify_project_api_token()


def require_scope(principal: TokenPrincipal, scope: str) -> None:
    """Raise InsufficientScopeError unless the principal's token grants `scope`."""
    if "*" in principal.scopes or scope in principal.scopes:
        return
    raise InsufficientScopeError(f"Token lacks required scope '{scope}'.")


def assert_token_workspace_access(principal: TokenPrincipal, workspace_id: int | None) -> None:
    """404s when a workspace-scoped token reaches a resource outside its workspace.

    Master-key callers (principal.workspace_id is None) bypass this check.
    """
    if principal.workspace_id is None:
        return
    from fastapi import HTTPException

    if workspace_id is None or principal.workspace_id != workspace_id:
        raise HTTPException(status_code=404, detail="Not found")


def assert_token_project_access(
    principal: TokenPrincipal, project_id: int | None, session: "Session | None" = None
) -> None:
    """404s (not 401) when a workspace-scoped token reaches a project outside its workspace.

    Resolves the project's workspace and compares it to the caller's workspace.
    Master-key callers (principal.workspace_id is None) bypass this check.
    If ``session`` is omitted a short-lived one is opened to resolve the mapping.
    """
    if principal.workspace_id is None:
        return
    from fastapi import HTTPException

    if project_id is None:
        raise HTTPException(status_code=404, detail="Not found")

    from models import Project

    if session is not None:
        row = session.get(Project, project_id)
    else:
        from database import engine

        with Session(engine) as s:
            row = s.get(Project, project_id)
    if row is None or row.workspace_id != principal.workspace_id:
        raise HTTPException(status_code=404, detail="Not found")
