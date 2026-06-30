"""Progress Reviewer check-ins and plan adjustment proposals."""

from __future__ import annotations

from typing import Any

from fastapi import HTTPException
from sqlmodel import Session, select

from action_orchestrator.engine import decide_actions
from action_orchestrator.registry import list_actions
from assistant.models import AssistantReview
from assistant.services.assistant_service import ensure_assistant_board
from time_utils import utc_now_naive
from todos.models import TodoItem
from todos.schemas import PlannedActionOut
from todos.seeds import REVIEWER_ROLE_PROMPT, get_todo_action_set_id


REVIEWER_PROMPT = REVIEWER_ROLE_PROMPT


def _compute_stats(items: list[TodoItem]) -> dict[str, Any]:
    now = utc_now_naive()
    done = [i for i in items if i.status == "done"]
    overdue = [i for i in items if i.due_at and i.due_at < now and i.status != "done"]
    habits = [i for i in items if i.item_kind == "habit"]
    habits_done = [i for i in habits if i.status == "done"]
    difficulties = []
    for i in done:
        comp = i.get_completion()
        if comp.get("difficulty"):
            difficulties.append(comp["difficulty"])
    avg_time = []
    for i in done:
        comp = i.get_completion()
        if comp.get("time_spent_minutes"):
            avg_time.append(comp["time_spent_minutes"])

    return {
        "total_items": len(items),
        "done_count": len(done),
        "active_count": len(items) - len(done),
        "overdue_count": len(overdue),
        "completion_rate": round(len(done) / len(items), 2) if items else 0,
        "habits_total": len(habits),
        "habits_done": len(habits_done),
        "overdue_titles": [i.title for i in overdue[:10]],
        "difficulty_breakdown": difficulties,
        "avg_time_spent_minutes": round(sum(avg_time) / len(avg_time), 1) if avg_time else None,
    }


async def run_review(
    session: Session,
    project_id: int,
    *,
    model: str | None = None,
) -> dict[str, Any]:
    board = ensure_assistant_board(session, project_id)
    items = session.exec(
        select(TodoItem).where(TodoItem.board_id == board.id)
    ).all()
    stats = _compute_stats(items)

    action_set_id = get_todo_action_set_id(session)
    if not action_set_id:
        raise HTTPException(status_code=400, detail="Action set not configured")

    from action_orchestrator.registry import list_actions

    from assistant.services.user_profile_service import get_all_profiles

    actions = list_actions(session, action_set_id)
    context: dict[str, Any] = {
        "reviewer_prompt": REVIEWER_PROMPT,
        "stats": stats,
        "board_id": board.id,
        "user_domain_profiles": get_all_profiles(session, project_id),
        "items_summary": [
            {
                "id": i.id,
                "title": i.title,
                "status": i.status,
                "due_at": i.due_at.isoformat() if i.due_at else None,
                "item_kind": i.item_kind,
                "completion": i.get_completion(),
            }
            for i in items[:30]
        ],
    }

    llm_model = model or board.default_model or "gemma4:31b-cloud"
    goal = (
        "Review the user's progress and propose plan adjustments. "
        "Focus on overdue items, habit consistency, and sustainable next steps."
    )
    planned, thought = await decide_actions(
        goal=goal,
        context=context,
        actions=actions,
        history=None,
        llm_model=llm_model,
    )
    planned_out = [
        PlannedActionOut(
            action_id=a.action_id,
            name=a.name,
            parameters=a.parameters,
            confidence=a.confidence,
            reasoning=a.reasoning,
        )
        for a in planned
    ]

    now = utc_now_naive()
    review = AssistantReview(
        project_id=project_id,
        status="pending",
        summary=thought or "Progress review complete.",
        created_at=now,
        updated_at=now,
    )
    review.set_stats(stats)
    review.set_proposed_actions([p.model_dump() for p in planned_out])
    session.add(review)
    session.commit()
    session.refresh(review)

    return {
        "review_id": review.id,
        "status": review.status,
        "summary": review.summary,
        "stats": stats,
        "proposed_actions": [p.model_dump() for p in planned_out],
    }


def dismiss_review(session: Session, review_id: int) -> dict[str, Any]:
    review = session.get(AssistantReview, review_id)
    if not review:
        raise HTTPException(status_code=404, detail="Review not found")
    if review.status == "applied":
        raise HTTPException(status_code=400, detail="Review already applied")
    if review.status == "dismissed":
        return {"review_id": review.id, "status": review.status}

    review.status = "dismissed"
    review.updated_at = utc_now_naive()
    session.add(review)
    session.commit()
    session.refresh(review)
    return {"review_id": review.id, "status": review.status}


def apply_review(
    session: Session,
    review_id: int,
    actions: list[PlannedActionOut] | None = None,
) -> dict[str, Any]:
    from assistant.services.board_action_apply import apply_board_actions
    from assistant.services.assistant_service import ensure_assistant_board

    review = session.get(AssistantReview, review_id)
    if not review:
        raise HTTPException(status_code=404, detail="Review not found")
    if review.status == "applied":
        raise HTTPException(status_code=400, detail="Review already applied")
    if review.status == "dismissed":
        raise HTTPException(status_code=400, detail="Review was dismissed")

    board = ensure_assistant_board(session, review.project_id)
    if actions:
        to_apply = actions
    else:
        raw = review.get_proposed_actions()
        to_apply = [PlannedActionOut(**a) for a in raw]

    result = apply_board_actions(session, board.id, to_apply)
    review.status = "applied"
    review.updated_at = utc_now_naive()
    session.add(review)
    session.commit()

    return {
        "review_id": review.id,
        "status": review.status,
        "applied": result.applied,
        "skipped": result.skipped,
        "created_items": [i.model_dump() for i in result.created_items],
        "updated_items": [i.model_dump() for i in result.updated_items],
    }


def get_pending_reviews(session: Session, project_id: int) -> list[dict[str, Any]]:
    rows = session.exec(
        select(AssistantReview)
        .where(
            AssistantReview.project_id == project_id,
            AssistantReview.status == "pending",
        )
        .order_by(AssistantReview.created_at.desc())
    ).all()
    return [
        {
            "id": r.id,
            "status": r.status,
            "summary": r.summary,
            "stats": r.get_stats(),
            "proposed_actions": r.get_proposed_actions(),
            "created_at": r.created_at.isoformat(),
        }
        for r in rows
    ]
