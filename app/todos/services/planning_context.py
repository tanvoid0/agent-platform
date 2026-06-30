"""Project-scoped planning preferences (server authority)."""

from __future__ import annotations

import json
from typing import Any

from fastapi import HTTPException
from sqlmodel import Session

from models import Project
from todos.models import TodoBoard


def _parse_prefs(raw: str | None) -> dict[str, Any]:
    if not raw:
        return {}
    try:
        data = json.loads(raw)
        return dict(data) if isinstance(data, dict) else {}
    except json.JSONDecodeError:
        return {}


def get_planning_context(session: Session, project_id: int) -> dict[str, Any]:
    project = session.get(Project, project_id)
    if not project:
        raise HTTPException(status_code=404, detail="Project not found")
    prefs = _parse_prefs(project.planning_prefs_json)
    last_board_id = project.last_todo_board_id
    if last_board_id is not None:
        board = session.get(TodoBoard, last_board_id)
        if not board or board.project_id != project_id:
            last_board_id = None
    return {
        "project_id": project_id,
        "last_todo_board_id": last_board_id,
        "onboarding_dismissed": bool(prefs.get("onboarding_dismissed")),
    }


def patch_planning_context(
    session: Session,
    project_id: int,
    *,
    last_todo_board_id: int | None = None,
    last_todo_board_set: bool = False,
    onboarding_dismissed: bool | None = None,
) -> dict[str, Any]:
    project = session.get(Project, project_id)
    if not project:
        raise HTTPException(status_code=404, detail="Project not found")

    if last_todo_board_set:
        if last_todo_board_id is not None:
            board = session.get(TodoBoard, last_todo_board_id)
            if not board:
                raise HTTPException(status_code=404, detail="Board not found")
            if board.project_id is not None and board.project_id != project_id:
                raise HTTPException(status_code=400, detail="Board belongs to another project")
        project.last_todo_board_id = last_todo_board_id

    if onboarding_dismissed is not None:
        prefs = _parse_prefs(project.planning_prefs_json)
        prefs["onboarding_dismissed"] = onboarding_dismissed
        project.planning_prefs_json = json.dumps(prefs, ensure_ascii=False)

    session.add(project)
    session.commit()
    session.refresh(project)
    return get_planning_context(session, project_id)


def record_board_visit(session: Session, board_id: int) -> None:
    board = session.get(TodoBoard, board_id)
    if not board or board.project_id is None:
        return
    patch_planning_context(
        session,
        board.project_id,
        last_todo_board_id=board_id,
        last_todo_board_set=True,
    )
