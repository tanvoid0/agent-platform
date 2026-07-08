"""Spawn a full DAG process from a todo item."""

from __future__ import annotations

from fastapi import HTTPException
from sqlmodel import Session

from models import Process, Project, TeamTemplate
from team_schema import (
    build_process_team_snapshot,
    parse_team_roster_json,
    resolved_team_color,
    with_default_accents,
)
from time_utils import utc_now_naive
from todos.models import TodoBoard, TodoItem
from todos.schemas import ItemOut, SpawnProcessResponse
from todos.services.board_service import _item_out
from todos.services.item_events import append_item_event


def spawn_process_for_item(
    session: Session,
    item_id: int,
    *,
    team_template_id: int,
    goal: str | None,
    auto_approve: bool = False,
) -> SpawnProcessResponse:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")

    tmpl = session.get(TeamTemplate, team_template_id)
    if not tmpl:
        raise HTTPException(status_code=404, detail="Team template not found")

    board = session.get(TodoBoard, item.board_id)
    project_id = board.project_id if board else None
    if project_id is not None:
        proj = session.get(Project, project_id)
        if not proj:
            raise HTTPException(status_code=404, detail="Project not found")

    stable_key = str(tmpl.id)
    team_color = resolved_team_color(tmpl.color, stable_key)
    roster = with_default_accents(
        parse_team_roster_json(tmpl.roster_json),
        team_color,
        stable_key=stable_key,
    )
    team_snapshot_json = build_process_team_snapshot(
        tmpl.id,
        tmpl.name,
        tmpl.description,
        team_color,
        roster,
    )
    proc_goal = (goal or "").strip() or f"{item.title}\n\n{item.description}".strip()

    proc = Process(
        goal=proc_goal,
        team_template_id=team_template_id,
        team_snapshot_json=team_snapshot_json,
        project_id=project_id,
        status="pending",
    )
    session.add(proc)
    session.commit()
    session.refresh(proc)

    item.linked_process_id = proc.id
    item.updated_at = utc_now_naive()
    session.add(item)
    session.commit()
    session.refresh(item)

    append_item_event(
        session,
        item_id,
        "process_spawned",
        {
            "process_id": proc.id,
            "team_template_id": team_template_id,
            "goal": proc_goal,
            "auto_approve": auto_approve,
        },
    )

    return SpawnProcessResponse(
        process_id=proc.id,
        status=proc.status,
        item=_item_out(item),
        auto_approve=auto_approve,
        note="Process created. Start planning via POST /api/v1/processes/{id}/sync or the process UI.",
    )
