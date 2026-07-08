"""Archive a workspace and cascade to tokens, teams, and access."""

from __future__ import annotations

from fastapi import HTTPException
from sqlmodel import Session, select, update

from models import ApiToken, Process, TeamTemplate, Workspace
from time_utils import utc_now_naive

ARCHIVE_REASON = "Workspace archived"


def require_active_workspace(session: Session, workspace_id: int, name: str = "Workspace") -> Workspace:
    row = session.get(Workspace, workspace_id)
    if row is None or row.archived_at is not None:
        raise HTTPException(status_code=404, detail=f"{name} not found")
    return row


def archive_workspace(session: Session, workspace: Workspace) -> dict:
    if workspace.archived_at is not None:
        raise HTTPException(status_code=409, detail="Workspace is already archived")
    if workspace.slug == "default":
        raise HTTPException(status_code=400, detail="Cannot archive the Default workspace")

    now = utc_now_naive()

    tokens = session.exec(
        select(ApiToken).where(
            ApiToken.workspace_id == workspace.id,
            ApiToken.status != "revoked",
        )
    ).all()
    for tok in tokens:
        tok.status = "revoked"
        tok.revoked_at = now
        tok.revoked_reason = ARCHIVE_REASON
        tok.updated_at = now
        session.add(tok)

    teams = session.exec(
        select(TeamTemplate).where(TeamTemplate.workspace_id == workspace.id)
    ).all()
    for team in teams:
        session.exec(
            update(Process).where(Process.team_template_id == team.id).values(team_template_id=None)
        )
        session.delete(team)

    workspace.archived_at = now
    workspace.updated_at = now
    session.add(workspace)
    session.commit()

    return {
        "ok": True,
        "archived_at": now.isoformat(),
        "tokens_revoked": len(tokens),
        "teams_removed": len(teams),
    }
