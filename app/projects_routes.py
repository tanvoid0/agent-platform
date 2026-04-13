"""CRUD for projects (user-facing process grouping)."""

from __future__ import annotations

from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, ConfigDict, Field
from sqlmodel import Session, select

from database import get_session
from models import Process, Project
from workspace_service import delete_project_workspace

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


@router.delete("/{project_id}")
def delete_project(project_id: int, session: Session = Depends(get_session)):
    row = session.get(Project, project_id)
    if not row:
        raise HTTPException(status_code=404, detail="Project not found")
    for proc in session.exec(select(Process).where(Process.project_id == project_id)).all():
        proc.project_id = None
        session.add(proc)
    session.delete(row)
    session.commit()
    delete_project_workspace(project_id)
    return {"ok": True}
