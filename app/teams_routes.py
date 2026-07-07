"""CRUD for team templates (planner roster hints)."""

from __future__ import annotations

from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, ConfigDict, Field
from sqlmodel import Session, select, update

from api_tokens.auth import TokenPrincipal, require_valid_token
from crud_helpers import require_one
from database import get_session
from models import Process, TeamTemplate, Workspace
from schema_converter import to_schemas
from schema_fields import ResourceName, ResourceDescription, ResourceColor, ResourceCategory
from team_schema import TeamRoster, parse_team_roster_json, roster_to_json
from time_utils import utc_now_naive

router = APIRouter(prefix="/teams", tags=["teams"])


def _assert_team_visible(principal: TokenPrincipal, row: TeamTemplate) -> None:
    """Workspace tokens see global (NULL) + their own templates; master sees all."""
    if principal.workspace_id is None:
        return
    if row.workspace_id is not None and row.workspace_id != principal.workspace_id:
        raise HTTPException(status_code=404, detail="Team template not found")


def _assert_team_owned(principal: TokenPrincipal, row: TeamTemplate) -> None:
    """Mutations: master may touch any; a workspace token may touch only its own
    templates (never global or another workspace's)."""
    if principal.workspace_id is None:
        return
    if row.workspace_id != principal.workspace_id:
        raise HTTPException(status_code=404, detail="Team template not found")


class TeamTemplateCreate(BaseModel):
    name: str = ResourceName
    description: str | None = ResourceDescription
    color: str | None = ResourceColor
    category: str | None = ResourceCategory
    roster: TeamRoster
    # Master-key only: NULL creates a global template, a value creates one owned
    # by that workspace. Ignored for workspace-scoped tokens (forced to their own).
    workspace_id: int | None = None


class TeamTemplateUpdate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str | None = Field(default=None, min_length=1, max_length=256)
    description: str | None = ResourceDescription
    color: str | None = ResourceColor
    category: str | None = ResourceCategory
    roster: TeamRoster | None = None


class TeamTemplateOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    workspace_id: int | None
    name: str
    description: str | None
    color: str | None
    category: str | None
    roster: TeamRoster
    role_count: int = Field(ge=0)
    created_at: datetime
    updated_at: datetime


class TeamTemplateSummary(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    workspace_id: int | None
    name: str
    description: str | None
    color: str | None
    category: str | None
    role_count: int = Field(ge=0, description="Number of roles in the roster")
    created_at: datetime
    updated_at: datetime


def _role_count_from_row(row: TeamTemplate) -> int:
    try:
        return len(parse_team_roster_json(row.roster_json).roles)
    except ValueError:
        return 0


def _row_to_out(row: TeamTemplate) -> TeamTemplateOut:
    roster = parse_team_roster_json(row.roster_json)
    return TeamTemplateOut(
        id=row.id,
        workspace_id=row.workspace_id,
        name=row.name,
        description=row.description,
        color=row.color,
        category=row.category,
        roster=roster,
        role_count=len(roster.roles),
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


@router.get("/")
def list_teams(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    q = select(TeamTemplate).order_by(TeamTemplate.id.asc())
    if principal.workspace_id is not None:
        # Global (NULL) + this workspace's own templates.
        q = q.where(
            (TeamTemplate.workspace_id.is_(None))
            | (TeamTemplate.workspace_id == principal.workspace_id)
        )
    rows = session.exec(q).all()
    return {
        "teams": [
            TeamTemplateSummary(
                id=r.id,
                workspace_id=r.workspace_id,
                name=r.name,
                description=r.description,
                color=r.color,
                category=r.category,
                role_count=_role_count_from_row(r),
                created_at=r.created_at,
                updated_at=r.updated_at,
            )
            for r in rows
        ]
    }


@router.post("/", status_code=201)
def create_team(
    req: TeamTemplateCreate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    if principal.workspace_id is not None:
        workspace_id = principal.workspace_id
    else:
        workspace_id = req.workspace_id
        if workspace_id is not None:
            require_one(session, Workspace, workspace_id, "Workspace")
    now = utc_now_naive()
    row = TeamTemplate(
        workspace_id=workspace_id,
        name=req.name.strip(),
        description=req.description.strip() if req.description else None,
        color=req.color.strip() if req.color else None,
        category=req.category.strip() if req.category else None,
        roster_json=roster_to_json(req.roster),
        created_at=now,
        updated_at=now,
    )
    session.add(row)
    session.commit()
    session.refresh(row)
    return _row_to_out(row)


@router.get("/{team_id}")
def get_team(
    team_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    row = require_one(session, TeamTemplate, team_id, "Team template")
    _assert_team_visible(principal, row)
    return _row_to_out(row)


@router.patch("/{team_id}")
def update_team(
    team_id: int,
    req: TeamTemplateUpdate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    row = require_one(session, TeamTemplate, team_id, "Team template")
    _assert_team_owned(principal, row)
    patch = req.model_dump(exclude_unset=True)
    if req.name is not None:
        row.name = req.name.strip()
    if req.description is not None:
        row.description = req.description.strip() if req.description else None
    if req.color is not None:
        row.color = req.color.strip() if req.color else None
    if "category" in patch:
        raw = patch.get("category")
        row.category = raw.strip() if isinstance(raw, str) and raw.strip() else None
    if req.roster is not None:
        row.roster_json = roster_to_json(req.roster)
    row.updated_at = utc_now_naive()
    session.add(row)
    session.commit()
    session.refresh(row)
    return _row_to_out(row)


@router.delete("/{team_id}")
def delete_team(
    team_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    row = require_one(session, TeamTemplate, team_id, "Team template")
    _assert_team_owned(principal, row)
    session.exec(update(Process).where(Process.team_template_id == team_id).values(team_template_id=None))
    session.delete(row)
    session.commit()
    return {"ok": True}
