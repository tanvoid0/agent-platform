"""Dashboard-only CRUD for project-scoped API tokens.

Master-key callers only — a project token must never be usable to mint or
revoke other tokens, so every endpoint here rejects project-scoped principals.
"""

from __future__ import annotations

from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, ConfigDict, Field
from sqlmodel import Session, select

from api_tokens.auth import TokenPrincipal, require_valid_token
from api_tokens.token_service import generate_token
from crud_helpers import require_one
from database import get_session
from models import ApiToken, ApiTokenUsageDaily, Project
from schema_converter import to_schemas

router = APIRouter(prefix="/projects/{project_id}/api-tokens", tags=["api-tokens"])


def _require_dashboard_caller(principal: TokenPrincipal) -> None:
    """Only the master key (unrestricted principal) may manage tokens."""
    if principal.project_id is not None:
        raise HTTPException(status_code=403, detail="API tokens cannot be managed using an API token.")


class ApiTokenCreate(BaseModel):
    name: str = Field(min_length=1, max_length=256)
    scopes: list[str] = Field(default_factory=list)
    expires_at: datetime | None = None
    rate_limit_per_minute: int | None = Field(default=None, ge=1)


class ApiTokenActionBody(BaseModel):
    model_config = ConfigDict(extra="ignore")

    reason: str | None = Field(default=None, max_length=512)


class ApiTokenOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    project_id: int
    name: str
    prefix: str
    scopes: list[str]
    status: str
    rate_limit_per_minute: int | None
    expires_at: datetime | None
    last_used_at: datetime | None
    revoked_at: datetime | None
    revoked_reason: str | None
    held_reason: str | None
    total_requests: int
    total_errors: int
    total_tokens: int
    total_cost: float
    created_at: datetime
    updated_at: datetime


class ApiTokenCreateOut(ApiTokenOut):
    token: str  # raw token, shown once


class ApiTokenUsageDailyOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    usage_date: str
    request_count: int
    error_count: int
    total_tokens: int
    total_cost: float


def _to_out(row: ApiToken) -> ApiTokenOut:
    return ApiTokenOut(
        id=row.id,
        project_id=row.project_id,
        name=row.name,
        prefix=row.prefix,
        scopes=row.scopes,
        status=row.status,
        rate_limit_per_minute=row.rate_limit_per_minute,
        expires_at=row.expires_at,
        last_used_at=row.last_used_at,
        revoked_at=row.revoked_at,
        revoked_reason=row.revoked_reason,
        held_reason=row.held_reason,
        total_requests=row.total_requests,
        total_errors=row.total_errors,
        total_tokens=row.total_tokens,
        total_cost=row.total_cost,
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


def _require_token(session: Session, project_id: int, token_id: int) -> ApiToken:
    row = require_one(session, ApiToken, token_id, "API token")
    if row.project_id != project_id:
        raise HTTPException(status_code=404, detail="API token not found")
    return row


@router.post(
    "/",
    status_code=201,
    response_model=ApiTokenCreateOut,
    summary="Create an API token for a project",
    description=(
        "Mints a new opaque bearer token scoped to this project. The raw token is returned "
        "once in this response and is never retrievable again — only its prefix and metadata "
        "are stored. Requires the master key (a project token cannot mint other tokens)."
    ),
)
def create_api_token(
    project_id: int,
    req: ApiTokenCreate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_dashboard_caller(principal)
    require_one(session, Project, project_id, "Project")

    full_token, prefix, token_hash = generate_token()
    now = datetime.utcnow()
    row = ApiToken(
        project_id=project_id,
        name=req.name.strip(),
        prefix=prefix,
        token_hash=token_hash,
        status="active",
        rate_limit_per_minute=req.rate_limit_per_minute,
        expires_at=req.expires_at,
        created_at=now,
        updated_at=now,
    )
    row.scopes = req.scopes
    session.add(row)
    session.commit()
    session.refresh(row)

    out = _to_out(row)
    return ApiTokenCreateOut(**out.model_dump(), token=full_token)


@router.get(
    "/",
    response_model=dict,
    summary="List API tokens for a project",
    description="Returns token metadata (prefix, status, scopes, usage totals) — never the raw token value.",
)
def list_api_tokens(
    project_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_dashboard_caller(principal)
    require_one(session, Project, project_id, "Project")
    rows = session.exec(
        select(ApiToken).where(ApiToken.project_id == project_id).order_by(ApiToken.id.desc())
    ).all()
    return {"tokens": to_schemas(rows, ApiTokenOut)}


@router.get("/{token_id}", response_model=ApiTokenOut, summary="Get one API token's metadata and lifetime usage totals")
def get_api_token(
    project_id: int,
    token_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_dashboard_caller(principal)
    row = _require_token(session, project_id, token_id)
    return _to_out(row)


@router.get(
    "/{token_id}/usage",
    response_model=dict,
    summary="Get per-day usage rollup for one API token",
    description="Requests, tokens, cost, and errors per UTC day, optionally bounded by from_date/to_date (YYYY-MM-DD).",
)
def get_api_token_usage(
    project_id: int,
    token_id: int,
    from_date: str | None = None,
    to_date: str | None = None,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_dashboard_caller(principal)
    _require_token(session, project_id, token_id)

    q = select(ApiTokenUsageDaily).where(ApiTokenUsageDaily.token_id == token_id)
    if from_date:
        q = q.where(ApiTokenUsageDaily.usage_date >= from_date)
    if to_date:
        q = q.where(ApiTokenUsageDaily.usage_date <= to_date)
    rows = session.exec(q.order_by(ApiTokenUsageDaily.usage_date.asc())).all()
    return {"daily": to_schemas(rows, ApiTokenUsageDailyOut)}


@router.post(
    "/{token_id}/revoke",
    response_model=ApiTokenOut,
    summary="Permanently revoke an API token",
    description="Irreversible. Subsequent requests using this token get 401 TOKEN_REVOKED.",
)
def revoke_api_token(
    project_id: int,
    token_id: int,
    req: ApiTokenActionBody,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_dashboard_caller(principal)
    row = _require_token(session, project_id, token_id)
    row.status = "revoked"
    row.revoked_at = datetime.utcnow()
    row.revoked_reason = req.reason
    row.updated_at = datetime.utcnow()
    session.add(row)
    session.commit()
    session.refresh(row)
    return _to_out(row)


@router.post(
    "/{token_id}/hold",
    response_model=ApiTokenOut,
    summary="Temporarily suspend an API token",
    description="Reversible via /unhold. Requests using this token get 403 TOKEN_HELD until unheld.",
)
def hold_api_token(
    project_id: int,
    token_id: int,
    req: ApiTokenActionBody,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_dashboard_caller(principal)
    row = _require_token(session, project_id, token_id)
    if row.status == "revoked":
        raise HTTPException(status_code=409, detail="Cannot hold a revoked token")
    row.status = "held"
    row.held_reason = req.reason
    row.updated_at = datetime.utcnow()
    session.add(row)
    session.commit()
    session.refresh(row)
    return _to_out(row)


@router.post(
    "/{token_id}/unhold",
    response_model=ApiTokenOut,
    summary="Restore a held API token to active",
)
def unhold_api_token(
    project_id: int,
    token_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_dashboard_caller(principal)
    row = _require_token(session, project_id, token_id)
    if row.status != "held":
        raise HTTPException(status_code=409, detail="Token is not on hold")
    row.status = "active"
    row.held_reason = None
    row.updated_at = datetime.utcnow()
    session.add(row)
    session.commit()
    session.refresh(row)
    return _to_out(row)
