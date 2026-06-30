"""Reset Personal Assistant workspace to a fresh state for a project."""

from __future__ import annotations

from fastapi import HTTPException
from sqlmodel import Session, select

from assistant.models import AssistantChatThread, AssistantReview
from assistant.services.assistant_chat import create_chat_thread
from assistant.services.assistant_service import ensure_assistant_board
from models import Project
from time_utils import utc_now_naive
from todos.models import TodoBoard, TodoCategory, TodoItem, TodoItemEvent


def _purge_todo_board(session: Session, board_id: int) -> None:
    """Remove board rows and dependents (events are not cascaded on item delete)."""
    items = session.exec(select(TodoItem).where(TodoItem.board_id == board_id)).all()
    if items:
        item_ids = [i.id for i in items if i.id is not None]
        if item_ids:
            events = session.exec(
                select(TodoItemEvent).where(TodoItemEvent.item_id.in_(item_ids))
            ).all()
            for event in events:
                session.delete(event)
        for item in sorted(items, key=lambda i: (i.parent_item_id is None, i.id or 0)):
            session.delete(item)

    categories = session.exec(
        select(TodoCategory).where(TodoCategory.board_id == board_id)
    ).all()
    for category in categories:
        session.delete(category)

    board = session.get(TodoBoard, board_id)
    if board:
        session.delete(board)


def reset_assistant_workspace(session: Session, project_id: int) -> dict[str, int]:
    """
    Delete the project's assistant board (tasks, categories, item events), all assistant
    chat threads, and reviews; then create a fresh board and chat thread.

    Domain profiles (user profile data) are preserved — clear those only from the profile page.
    """
    project = session.get(Project, project_id)
    if not project:
        raise HTTPException(status_code=404, detail="Project not found")

    board_id = project.assistant_board_id
    now = utc_now_naive()

    project.assistant_board_id = None
    if board_id is not None and project.last_todo_board_id == board_id:
        project.last_todo_board_id = None
    project.planning_prefs_json = None
    project.updated_at = now
    session.add(project)
    session.flush()

    for row in session.exec(
        select(AssistantChatThread).where(AssistantChatThread.project_id == project_id)
    ).all():
        session.delete(row)

    for row in session.exec(
        select(AssistantReview).where(AssistantReview.project_id == project_id)
    ).all():
        session.delete(row)

    if board_id is not None:
        _purge_todo_board(session, board_id)

    session.commit()

    board = ensure_assistant_board(session, project_id)
    thread = create_chat_thread(session, project_id)

    return {
        "project_id": project_id,
        "board_id": board.id,
        "thread_id": thread.id,
    }
