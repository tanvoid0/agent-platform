"""Deprecated project-scoped token routes — resolve workspace from project_id.

One-release alias; use ``/workspaces/{workspace_id}/api-tokens`` instead.
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, Response
from sqlmodel import Session

from api_tokens.auth import TokenPrincipal, require_valid_token
from api_tokens.routes import (
    ApiTokenActionBody,
    ApiTokenCreate,
    ApiTokenCreateOut,
    ApiTokenOut,
    ApiTokenUpdate,
    create_api_token,
    get_api_token,
    get_api_token_usage,
    hold_api_token,
    list_api_tokens,
    revoke_api_token,
    unhold_api_token,
    update_api_token,
)
from crud_helpers import require_one
from database import get_session
from models import Project

router = APIRouter(prefix="/projects/{project_id}/api-tokens", tags=["api-tokens"], deprecated=True)

_DEPRECATION = "true"
_SUCCESSOR = '</api/v1/workspaces/{workspace_id}/api-tokens>; rel="successor-version"'


def _deprecate(response: Response) -> None:
    response.headers["Deprecation"] = _DEPRECATION
    response.headers["Link"] = _SUCCESSOR


def _workspace_id(session: Session, project_id: int) -> int:
    row = require_one(session, Project, project_id, "Project")
    return row.workspace_id


@router.post("/", status_code=201, response_model=ApiTokenCreateOut)
def create_api_token_legacy(
    project_id: int,
    req: ApiTokenCreate,
    response: Response,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return create_api_token(_workspace_id(session, project_id), req, session, principal)


@router.get("/", response_model=dict)
def list_api_tokens_legacy(
    project_id: int,
    response: Response,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return list_api_tokens(_workspace_id(session, project_id), session, principal)


@router.get("/{token_id}", response_model=ApiTokenOut)
def get_api_token_legacy(
    project_id: int,
    token_id: int,
    response: Response,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return get_api_token(_workspace_id(session, project_id), token_id, session, principal)


@router.patch("/{token_id}", response_model=ApiTokenOut)
def update_api_token_legacy(
    project_id: int,
    token_id: int,
    req: ApiTokenUpdate,
    response: Response,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return update_api_token(_workspace_id(session, project_id), token_id, req, session, principal)


@router.get("/{token_id}/usage", response_model=dict)
def get_api_token_usage_legacy(
    project_id: int,
    token_id: int,
    response: Response,
    from_date: str | None = None,
    to_date: str | None = None,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return get_api_token_usage(
        _workspace_id(session, project_id),
        token_id,
        from_date,
        to_date,
        session,
        principal,
    )


@router.post("/{token_id}/revoke", response_model=ApiTokenOut)
def revoke_api_token_legacy(
    project_id: int,
    token_id: int,
    req: ApiTokenActionBody,
    response: Response,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return revoke_api_token(_workspace_id(session, project_id), token_id, req, session, principal)


@router.post("/{token_id}/hold", response_model=ApiTokenOut)
def hold_api_token_legacy(
    project_id: int,
    token_id: int,
    req: ApiTokenActionBody,
    response: Response,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return hold_api_token(_workspace_id(session, project_id), token_id, req, session, principal)


@router.post("/{token_id}/unhold", response_model=ApiTokenOut)
def unhold_api_token_legacy(
    project_id: int,
    token_id: int,
    response: Response,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _deprecate(response)
    return unhold_api_token(_workspace_id(session, project_id), token_id, session, principal)
