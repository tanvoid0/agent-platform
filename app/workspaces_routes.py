"""CRUD for workspaces (top-level tenant) + /me/workspace resolver.

Workspace management is master-key only — a workspace-scoped token must never
create, rename, or delete tenants. `/me/workspace` is the one endpoint a
workspace token may call, so the Flow UI can resolve its tenant from the token
alone (no workspace id baked into `.env`).
"""

from __future__ import annotations

import re
from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import BaseModel, ConfigDict, Field
from sqlmodel import Session, select

from api_tokens.auth import TokenPrincipal, require_valid_token
from database import get_session
from models import Workspace
from schema_converter import to_schemas
from time_utils import utc_now_naive
from workspace_archive import archive_workspace, require_active_workspace

router = APIRouter(prefix="/workspaces", tags=["workspaces"])
me_router = APIRouter(prefix="/me", tags=["workspaces"])


def _require_master_key(principal: TokenPrincipal) -> None:
    """Only the master key (unrestricted principal) may manage workspaces."""
    if principal.workspace_id is not None:
        raise HTTPException(
            status_code=403, detail="Workspaces cannot be managed using an API token."
        )


def _slugify(value: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", value.strip().lower()).strip("-")
    return slug or "workspace"


class WorkspaceCreate(BaseModel):
    name: str = Field(min_length=1, max_length=256)
    slug: str | None = Field(default=None, min_length=1, max_length=128)
    description: str | None = Field(default=None, max_length=4096)


class WorkspaceUpdate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str | None = Field(default=None, min_length=1, max_length=256)
    description: str | None = Field(default=None, max_length=4096)


class WorkspaceOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    name: str
    slug: str
    description: str | None
    archived_at: datetime | None = None
    created_at: datetime
    updated_at: datetime


@router.get("/")
def list_workspaces(
    include_archived: bool = Query(default=False),
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_master_key(principal)
    q = select(Workspace).order_by(Workspace.id.asc())
    if not include_archived:
        q = q.where(Workspace.archived_at.is_(None))
    rows = session.exec(q).all()
    return {"workspaces": to_schemas(rows, WorkspaceOut)}


@router.post("/", status_code=201, response_model=WorkspaceOut)
def create_workspace(
    req: WorkspaceCreate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_master_key(principal)
    slug = _slugify(req.slug or req.name)
    if session.exec(select(Workspace).where(Workspace.slug == slug)).first():
        raise HTTPException(status_code=409, detail=f"Workspace slug '{slug}' already exists")
    now = utc_now_naive()
    row = Workspace(
        name=req.name.strip(),
        slug=slug,
        description=req.description.strip() if req.description else None,
        created_at=now,
        updated_at=now,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return WorkspaceOut.model_validate(row)


@router.get("/{workspace_id}", response_model=WorkspaceOut)
def get_workspace(
    workspace_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_master_key(principal)
    row = require_active_workspace(session, workspace_id)
    return WorkspaceOut.model_validate(row)


@router.patch("/{workspace_id}", response_model=WorkspaceOut)
def update_workspace(
    workspace_id: int,
    req: WorkspaceUpdate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_master_key(principal)
    row = require_active_workspace(session, workspace_id)
    if req.name is not None:
        row.name = req.name.strip()
    if req.description is not None:
        row.description = req.description.strip() if req.description else None
    row.updated_at = utc_now_naive()
    session.add(row)
    session.commit()
    session.refresh(row)
    return WorkspaceOut.model_validate(row)


@router.delete("/{workspace_id}")
def delete_workspace(
    workspace_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Archive workspace: revoke tokens, remove workspace-owned teams, hide tenant."""
    _require_master_key(principal)
    row = require_active_workspace(session, workspace_id)
    return archive_workspace(session, row)


@me_router.get("/workspace", response_model=WorkspaceOut)
def get_my_workspace(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Resolve the workspace bound to the caller's token.

    Master-key callers have no single workspace — 400 with a hint to pick one
    from `GET /workspaces`.
    """
    if principal.workspace_id is None:
        raise HTTPException(
            status_code=400,
            detail="Master key is not bound to a workspace; use GET /workspaces to list them.",
        )
    row = require_active_workspace(session, principal.workspace_id)
    return WorkspaceOut.model_validate(row)
