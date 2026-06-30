"""CRUD for projects (user-facing process grouping)."""

from __future__ import annotations

import json
from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, ConfigDict, Field
from sqlmodel import Session, select, update

from database import get_session
from models import Process, Project
from workspace_service import delete_project_workspace
from todos.services.planning_context import get_planning_context, patch_planning_context

router = APIRouter(prefix="/projects", tags=["projects"])


class ProjectCreate(BaseModel):
    name: str = Field(min_length=1, max_length=256)
    description: str | None = Field(default=None, max_length=4096)
    color: str | None = Field(default=None, max_length=32)


class ProjectUpdate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str | None = Field(default=None, min_length=1, max_length=256)
    description: str | None = Field(default=None, max_length=4096)
    color: str | None = Field(default=None, max_length=32)


class ProjectOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    name: str
    description: str | None
    color: str | None
    created_at: datetime
    updated_at: datetime


class ProjectSummary(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
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
def list_projects(session: Session = Depends(get_session)):
    rows = session.exec(select(Project).order_by(Project.id.asc())).all()
    return {
        "projects": [
            ProjectSummary(
                id=r.id,
                name=r.name,
                description=r.description,
                color=r.color,
                created_at=r.created_at,
                updated_at=r.updated_at,
            )
            for r in rows
        ]
    }


@router.post("/", status_code=201)
def create_project(req: ProjectCreate, session: Session = Depends(get_session)):
    now = datetime.utcnow()
    row = Project(
        name=req.name.strip(),
        description=req.description.strip() if req.description else None,
        color=req.color.strip() if req.color else None,
        created_at=now,
        updated_at=now,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return ProjectOut(
        id=row.id,
        name=row.name,
        description=row.description,
        color=row.color,
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


@router.get("/{project_id}")
def get_project(project_id: int, session: Session = Depends(get_session)):
    row = session.get(Project, project_id)
    if not row:
        raise HTTPException(status_code=404, detail="Project not found")
    return ProjectOut(
        id=row.id,
        name=row.name,
        description=row.description,
        color=row.color,
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


@router.patch("/{project_id}")
def update_project(
    project_id: int,
    req: ProjectUpdate,
    session: Session = Depends(get_session),
):
    row = session.get(Project, project_id)
    if not row:
        raise HTTPException(status_code=404, detail="Project not found")
    if req.name is not None:
        row.name = req.name.strip()
    if req.description is not None:
        row.description = req.description.strip() if req.description else None
    if req.color is not None:
        row.color = req.color.strip() if req.color else None
    row.updated_at = datetime.utcnow()
    session.add(row)
    session.commit()
    session.refresh(row)
    return ProjectOut(
        id=row.id,
        name=row.name,
        description=row.description,
        color=row.color,
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


@router.get("/{project_id}/workspace-state")
def get_project_workspace_state(project_id: int, session: Session = Depends(get_session)):
    row = session.get(Project, project_id)
    if not row:
        raise HTTPException(status_code=404, detail="Project not found")
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
):
    row = session.get(Project, project_id)
    if not row:
        raise HTTPException(status_code=404, detail="Project not found")
    row.workspace_payload_json = json.dumps(req.payload, ensure_ascii=False)
    row.updated_at = datetime.utcnow()
    session.add(row)
    session.commit()
    session.refresh(row)
    return ProjectWorkspaceStateOut(payload=req.payload, updated_at=row.updated_at)


@router.get("/{project_id}/planning-context", response_model=ProjectPlanningContextOut)
def get_project_planning_context(project_id: int, session: Session = Depends(get_session)):
    ctx = get_planning_context(session, project_id)
    return ProjectPlanningContextOut(**ctx)


@router.patch("/{project_id}/planning-context", response_model=ProjectPlanningContextOut)
def patch_project_planning_context(
    project_id: int,
    req: ProjectPlanningContextPatch,
    session: Session = Depends(get_session),
):
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


@router.delete("/{project_id}")
def delete_project(project_id: int, session: Session = Depends(get_session)):
    row = session.get(Project, project_id)
    if not row:
        raise HTTPException(status_code=404, detail="Project not found")
    session.exec(update(Process).where(Process.project_id == project_id).values(project_id=None))
    session.delete(row)
    session.commit()
    delete_project_workspace(project_id)
    return {"ok": True}
