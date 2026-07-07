"""CRUD for projects (user-facing process grouping)."""

from __future__ import annotations

import json
from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, ConfigDict, Field
from sqlmodel import Session, select, update

from api_tokens.auth import (
    TokenPrincipal,
    assert_token_project_access,
    assert_token_workspace_access,
    require_valid_token,
)
from crud_helpers import require_one
from database import get_session
from models import Process, Project, Workspace
from schema_converter import to_schemas
from schema_fields import ResourceName, ResourceDescription, ResourceColor
from workspace_service import delete_project_workspace
from todos.services.planning_context import get_planning_context, patch_planning_context
from time_utils import utc_now_naive

router = APIRouter(prefix="/projects", tags=["projects"])


class ProjectCreate(BaseModel):
    name: str = ResourceName
    description: str | None = ResourceDescription
    color: str | None = ResourceColor
    # Required for master-key callers (which span all workspaces); ignored for
    # workspace-scoped tokens, whose workspace is taken from the token.
    workspace_id: int | None = None


class ProjectUpdate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str | None = Field(default=None, min_length=1, max_length=256)
    description: str | None = ResourceDescription
    color: str | None = ResourceColor


class ProjectOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    workspace_id: int | None
    name: str
    description: str | None
    color: str | None
    created_at: datetime
    updated_at: datetime


class ProjectSummary(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    workspace_id: int | None
    name: str
    description: str | None
    color: str | None
    created_at: datetime
    updated_at: datetime


class ProjectWorkspaceStateOut(BaseModel):
    payload: dict | None = None
    updated_at: datetime | None = None


class ProjectWorkspaceStatePut(BaseModel):
    model_config = ConfigDict(extra="ignore")

    payload: dict = Field(default_factory=dict)


class ProjectPlanningContextOut(BaseModel):
    project_id: int
    last_todo_board_id: int | None
    onboarding_dismissed: bool = False


class ProjectPlanningContextPatch(BaseModel):
    model_config = ConfigDict(extra="ignore")

    last_todo_board_id: int | None = None
    onboarding_dismissed: bool | None = None


@router.get("/")
def list_projects(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    q = select(Project).order_by(Project.id.asc())
    if principal.workspace_id is not None:
        q = q.where(Project.workspace_id == principal.workspace_id)
    rows = session.exec(q).all()
    return {"projects": to_schemas(rows, ProjectSummary)}


@router.post("/", status_code=201)
def create_project(
    req: ProjectCreate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    if principal.workspace_id is not None:
        workspace_id = principal.workspace_id
    elif req.workspace_id is not None:
        workspace_id = req.workspace_id
    else:
        # Master key with no explicit workspace → the seeded Default tenant.
        default = session.exec(select(Workspace).where(Workspace.slug == "default")).first()
        if default is None:
            raise HTTPException(
                status_code=400,
                detail="workspace_id is required (no Default workspace exists).",
            )
        workspace_id = default.id
    require_one(session, Workspace, workspace_id, "Workspace")
    now = utc_now_naive()
    row = Project(
        workspace_id=workspace_id,
        name=req.name.strip(),
        description=req.description.strip() if req.description else None,
        color=req.color.strip() if req.color else None,
        created_at=now,
        updated_at=now,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return ProjectOut.model_validate(row)


@router.get("/{project_id}")
def get_project(
    project_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    row = require_one(session, Project, project_id, "Project")
    return ProjectOut.model_validate(row)


@router.patch("/{project_id}")
def update_project(
    project_id: int,
    req: ProjectUpdate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    row = require_one(session, Project, project_id, "Project")
    if req.name is not None:
        row.name = req.name.strip()
    if req.description is not None:
        row.description = req.description.strip() if req.description else None
    if req.color is not None:
        row.color = req.color.strip() if req.color else None
    row.updated_at = utc_now_naive()
    session.add(row)
    session.commit()
    session.refresh(row)
    return ProjectOut.model_validate(row)


@router.get("/{project_id}/workspace-state")
def get_project_workspace_state(
    project_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    row = require_one(session, Project, project_id, "Project")
    payload = None
    if row.workspace_payload_json:
        try:
            parsed = json.loads(row.workspace_payload_json)
            if isinstance(parsed, dict):
                payload = parsed
        except json.JSONDecodeError:
            payload = None
    return ProjectWorkspaceStateOut(payload=payload, updated_at=row.updated_at)


@router.put("/{project_id}/workspace-state")
def put_project_workspace_state(
    project_id: int,
    req: ProjectWorkspaceStatePut,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    row = require_one(session, Project, project_id, "Project")
    row.workspace_payload_json = json.dumps(req.payload, ensure_ascii=False)
    row.updated_at = utc_now_naive()
    session.add(row)
    session.commit()
    session.refresh(row)
    return ProjectWorkspaceStateOut(payload=req.payload, updated_at=row.updated_at)


@router.get("/{project_id}/planning-context", response_model=ProjectPlanningContextOut)
def get_project_planning_context(
    project_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    ctx = get_planning_context(session, project_id)
    return ProjectPlanningContextOut(**ctx)


@router.patch("/{project_id}/planning-context", response_model=ProjectPlanningContextOut)
def patch_project_planning_context(
    project_id: int,
    req: ProjectPlanningContextPatch,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    fields = req.model_dump(exclude_unset=True)
    last_set = "last_todo_board_id" in fields
    ctx = patch_planning_context(
        session,
        project_id,
        last_todo_board_id=fields.get("last_todo_board_id"),
        last_todo_board_set=last_set,
        onboarding_dismissed=fields.get("onboarding_dismissed"),
    )
    return ProjectPlanningContextOut(**ctx)


@router.get("/{project_id}/processes")
def list_project_processes(
    project_id: int,
    session: Session = Depends(get_session),
    limit: int = 50,
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    require_one(session, Project, project_id, "Project")
    q = (
        select(Process)
        .where(Process.project_id == project_id)
        .order_by(Process.id.desc())
    )
    rows = session.exec(q.limit(min(limit, 200))).all()
    return {"processes": rows}


@router.delete("/{project_id}")
def delete_project(
    project_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    assert_token_project_access(principal, project_id, session)
    row = require_one(session, Project, project_id, "Project")
    session.exec(update(Process).where(Process.project_id == project_id).values(project_id=None))
    session.delete(row)
    session.commit()
    delete_project_workspace(project_id)
    return {"ok": True}
