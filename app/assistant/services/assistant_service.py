"""Ensure assistant board and aggregate dashboard data."""

from __future__ import annotations

from datetime import datetime, timedelta

from fastapi import HTTPException
from sqlmodel import Session, select

from models import Project
from time_utils import utc_now_naive
from todos.models import TodoBoard, TodoCategory, TodoItem
from todos.schemas import BoardCreate, CategoryOut, ItemOut
from todos.services.board_service import _category_out, _item_out, create_board, get_board


ASSISTANT_BOARD_NAME = "Personal Assistant"
ASSISTANT_TEMPLATE_SLUG = "personal-assistant"


def ensure_assistant_board(session: Session, project_id: int) -> TodoBoard:
    project = session.get(Project, project_id)
    if not project:
        raise HTTPException(status_code=404, detail="Project not found")

    if project.assistant_board_id:
        board = session.get(TodoBoard, project.assistant_board_id)
        if board and board.project_id == project_id:
            return board

    existing = session.exec(
        select(TodoBoard).where(
            TodoBoard.project_id == project_id,
            TodoBoard.name == ASSISTANT_BOARD_NAME,
        )
    ).first()
    if existing:
        project.assistant_board_id = existing.id
        project.updated_at = utc_now_naive()
        session.add(project)
        session.commit()
        return existing

    board_out = create_board(
        session,
        BoardCreate(
            name=ASSISTANT_BOARD_NAME,
            description="Your personal planning board — agents organize, you execute.",
            template_slug=ASSISTANT_TEMPLATE_SLUG,
        ),
        project_id=project_id,
    )
    board = session.get(TodoBoard, board_out.id)
    if not board:
        raise HTTPException(status_code=500, detail="Failed to create assistant board")
    project.assistant_board_id = board.id
    project.updated_at = utc_now_naive()
    session.add(project)
    session.commit()
    session.refresh(board)
    return board


def _horizon_range(horizon: str, now: datetime) -> tuple[datetime, datetime]:
    start = now.replace(hour=0, minute=0, second=0, microsecond=0)
    if horizon == "day":
        end = start + timedelta(days=1)
    elif horizon == "week":
        end = start + timedelta(days=7)
    elif horizon == "month":
        end = start + timedelta(days=30)
    else:
        end = start + timedelta(days=1)
    return start, end


def _item_in_horizon(item: TodoItem, horizon: str, now: datetime) -> bool:
    if item.time_horizon == horizon:
        return True
    if item.time_horizon == "goal" and horizon == "month":
        return item.item_kind == "goal"
    start, end = _horizon_range(horizon, now)
    for dt in (item.scheduled_at, item.due_at):
        if dt and start <= dt < end:
            return True
    if horizon == "day" and item.status in ("in_progress", "plan") and not item.parent_item_id:
        if item.time_horizon in (None, "day", "week"):
            return True
    return False


def get_dashboard(
    session: Session,
    project_id: int,
    horizon: str = "day",
) -> dict:
    board = ensure_assistant_board(session, project_id)
    detail = get_board(session, board.id)
    now = utc_now_naive()
    start, end = _horizon_range(horizon, now)

    items = session.exec(
        select(TodoItem).where(TodoItem.board_id == board.id)
    ).all()

    filtered: list[TodoItem] = []
    overdue: list[TodoItem] = []
    habits_due: list[TodoItem] = []
    goals: list[TodoItem] = []

    for item in items:
        if item.item_kind == "goal" or item.time_horizon == "goal":
            goals.append(item)
        if item.item_kind == "habit" and item.status != "done":
            habits_due.append(item)
        if item.due_at and item.due_at < now and item.status != "done":
            overdue.append(item)
        if _item_in_horizon(item, horizon, now):
            filtered.append(item)

    subtasks_by_parent: dict[int, list[ItemOut]] = {}
    top_level: list[ItemOut] = []
    for item in filtered:
        out = _item_out(item)
        if item.parent_item_id:
            subtasks_by_parent.setdefault(item.parent_item_id, []).append(out)
        else:
            top_level.append(out)

    categories = session.exec(
        select(TodoCategory)
        .where(TodoCategory.board_id == board.id)
        .order_by(TodoCategory.sort_order.asc())
    ).all()

    done_count = sum(1 for i in items if i.status == "done")
    total_active = sum(1 for i in items if i.status != "done")

    return {
        "project_id": project_id,
        "board_id": board.id,
        "horizon": horizon,
        "range_start": start.isoformat(),
        "range_end": end.isoformat(),
        "categories": [_category_out(c) for c in categories],
        "items": top_level,
        "subtasks_by_parent": subtasks_by_parent,
        "overdue": [_item_out(i) for i in overdue],
        "habits_due": [_item_out(i) for i in habits_due],
        "goals": [_item_out(i) for i in goals],
        "stats": {
            "total_items": len(items),
            "done_count": done_count,
            "active_count": total_active,
            "overdue_count": len(overdue),
            "habits_due_count": len(habits_due),
        },
    }


def get_goals(session: Session, project_id: int) -> list[ItemOut]:
    board = ensure_assistant_board(session, project_id)
    rows = session.exec(
        select(TodoItem).where(
            TodoItem.board_id == board.id,
            TodoItem.item_kind == "goal",
        )
    ).all()
    return [_item_out(i) for i in rows]
