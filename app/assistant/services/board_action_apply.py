"""Apply board-level agent actions for the Personal Assistant."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from fastapi import HTTPException
from sqlmodel import Session

from time_utils import utc_now_naive
from todos.models import TODO_STATUSES, TodoItem
from todos.schemas import ItemCreate, ItemOut, PlannedActionOut
from todos.services.board_service import _item_out, create_item, update_item
from todos.services.server_actions import execute_trigger_webhook
from todos.schemas import ItemUpdate


class BoardApplyResult:
    def __init__(self) -> None:
        self.applied: list[str] = []
        self.skipped: list[str] = []
        self.guidance: list[str] = []
        self.created_items: list[ItemOut] = []
        self.updated_items: list[ItemOut] = []


def _as_str(v: Any) -> str | None:
    return v if isinstance(v, str) else None


def _as_int(v: Any) -> int | None:
    return v if isinstance(v, int) else None


def _parse_dt(v: Any) -> datetime | None:
    if not isinstance(v, str) or not v.strip():
        return None
    text = v.strip().replace("Z", "+00:00")
    try:
        return datetime.fromisoformat(text)
    except ValueError:
        return None


def apply_board_actions(
    session: Session,
    board_id: int,
    actions: list[PlannedActionOut],
) -> BoardApplyResult:
    result = BoardApplyResult()
    for action in actions:
        p = action.parameters or {}
        aid = action.action_id

        if aid == "create_item":
            title = _as_str(p.get("title"))
            if not title:
                result.skipped.append("create_item: missing title")
                continue
            status = _as_str(p.get("status")) or "plan"
            if status not in TODO_STATUSES:
                status = "plan"
            item_kind = _as_str(p.get("item_kind")) or "task"
            item = create_item(
                session,
                board_id,
                ItemCreate(
                    title=title,
                    description=_as_str(p.get("description")) or "",
                    status=status,
                    category_id=_as_int(p.get("category_id")),
                    priority=_as_int(p.get("priority")) or 0,
                    parent_item_id=_as_int(p.get("parent_item_id")),
                    due_at=_parse_dt(p.get("due_at")),
                    scheduled_at=_parse_dt(p.get("scheduled_at")),
                    time_horizon=_as_str(p.get("time_horizon")),
                    item_kind=item_kind,
                ),
            )
            result.created_items.append(item)
            result.applied.append(f"Created: {title}")

        elif aid == "create_habit":
            title = _as_str(p.get("title"))
            if not title:
                result.skipped.append("create_habit: missing title")
                continue
            recurrence = p.get("recurrence")
            rec_dict = recurrence if isinstance(recurrence, dict) else {"cadence": "daily"}
            item = create_item(
                session,
                board_id,
                ItemCreate(
                    title=title,
                    description=_as_str(p.get("description")) or "",
                    status="backlog",
                    category_id=_as_int(p.get("category_id")),
                    item_kind="habit",
                    time_horizon=_as_str(p.get("time_horizon")) or "day",
                    recurrence=rec_dict,
                ),
            )
            result.created_items.append(item)
            result.applied.append(f"Created habit: {title}")

        elif aid == "create_subtask_item":
            parent_id = _as_int(p.get("parent_item_id"))
            title = _as_str(p.get("title"))
            if not parent_id or not title:
                result.skipped.append("create_subtask_item: missing parent or title")
                continue
            parent = session.get(TodoItem, parent_id)
            if not parent or parent.board_id != board_id:
                result.skipped.append("create_subtask_item: invalid parent")
                continue
            item = create_item(
                session,
                board_id,
                ItemCreate(
                    title=title,
                    description=_as_str(p.get("description")) or "",
                    status="plan",
                    category_id=parent.category_id,
                    parent_item_id=parent_id,
                    due_at=_parse_dt(p.get("due_at")),
                    scheduled_at=_parse_dt(p.get("scheduled_at")),
                    time_horizon=parent.time_horizon or "week",
                ),
            )
            result.created_items.append(item)
            result.applied.append(f"Created subtask: {title}")

        elif aid == "propose_review":
            reason = _as_str(p.get("reason")) or "Progress review suggested"
            result.guidance.append(reason)
            focus = p.get("focus_areas")
            if isinstance(focus, list):
                result.guidance.extend(str(x) for x in focus)
            result.applied.append("Review proposed")

        elif aid == "ask_clarifying_questions":
            qs = p.get("questions")
            if isinstance(qs, list):
                result.guidance.extend(str(q) for q in qs if q is not None and str(q).strip())
            result.applied.append("Questions noted")

        elif aid == "suggest_next_steps":
            guidance = _as_str(p.get("guidance"))
            if guidance:
                result.guidance.append(guidance)
            steps = p.get("steps")
            if isinstance(steps, list):
                result.guidance.extend(str(s) for s in steps if s is not None and str(s).strip())
            result.applied.append("Guidance noted")

        elif aid == "present_planning_form":
            form = p.get("form")
            if not isinstance(form, dict):
                result.skipped.append("present_planning_form: invalid form")
                continue
            result.applied.append("Planning form ready for user")
            result.guidance.append(f"Form: {form.get('title', 'Details needed')}")

        elif aid == "store_user_profile":
            domain = _as_str(p.get("domain"))
            data = p.get("data")
            if not domain or not isinstance(data, dict):
                result.skipped.append("store_user_profile: invalid domain or data")
                continue
            from assistant.services.user_profile_service import merge_profile
            from todos.models import TodoBoard

            board_row = session.get(TodoBoard, board_id)
            if not board_row or board_row.project_id is None:
                result.skipped.append("store_user_profile: board not project-scoped")
                continue
            merge_profile(session, board_row.project_id, domain, data)
            result.applied.append(f"Saved {domain} profile")

        elif aid == "trigger_webhook":
            url = _as_str(p.get("webhook_url"))
            if not url:
                result.skipped.append("trigger_webhook: missing webhook_url")
                continue
            payload = p.get("payload") if isinstance(p.get("payload"), dict) else {}
            try:
                hook_result = execute_trigger_webhook(url, payload)
                result.applied.append(
                    f"Webhook {hook_result['status_code']}"
                    + (" OK" if hook_result["ok"] else " failed")
                )
            except (ValueError, Exception) as e:
                result.skipped.append(f"trigger_webhook: {e}")

        elif aid in {
            "move_item_status",
            "update_item",
            "add_subtask",
            "break_down_task",
            "schedule_item",
            "set_due_date",
            "log_completion",
            "adjust_plan",
            "present_planning_form",
            "export_markdown_checklist",
            "export_ics_event",
        }:
            item_id = _as_int(p.get("item_id"))
            if not item_id:
                result.skipped.append(f"{aid}: missing item_id")
                continue
            item = session.get(TodoItem, item_id)
            if not item or item.board_id != board_id:
                result.skipped.append(f"{aid}: item not on board")
                continue
            updated = _apply_item_action(session, item, aid, p)
            if updated:
                result.updated_items.append(updated)
                result.applied.append(f"{aid} on item {item_id}")
            else:
                result.skipped.append(f"{aid}: no change")

        else:
            result.skipped.append(f"Unknown action: {aid}")

    return result


def _apply_item_action(
    session: Session,
    item: TodoItem,
    aid: str,
    p: dict[str, Any],
) -> ItemOut | None:
    if aid == "move_item_status":
        status = _as_str(p.get("status"))
        if not status or status not in TODO_STATUSES:
            return None
        return update_item(session, item.id, ItemUpdate(status=status))

    if aid == "update_item":
        patch = ItemUpdate(
            title=_as_str(p.get("title")),
            description=p.get("description") if isinstance(p.get("description"), str) else None,
            priority=_as_int(p.get("priority")),
        )
        return update_item(session, item.id, patch)

    if aid == "schedule_item":
        scheduled = _parse_dt(p.get("scheduled_at"))
        if not scheduled:
            return None
        return update_item(
            session,
            item.id,
            ItemUpdate(
                scheduled_at=scheduled,
                time_horizon=_as_str(p.get("time_horizon")),
            ),
        )

    if aid == "set_due_date":
        due = _parse_dt(p.get("due_at"))
        if not due:
            return None
        return update_item(session, item.id, ItemUpdate(due_at=due))

    if aid == "adjust_plan":
        return update_item(
            session,
            item.id,
            ItemUpdate(
                title=_as_str(p.get("title")),
                description=p.get("description") if isinstance(p.get("description"), str) else None,
                due_at=_parse_dt(p.get("due_at")),
                scheduled_at=_parse_dt(p.get("scheduled_at")),
                time_horizon=_as_str(p.get("time_horizon")),
                status=_as_str(p.get("status")),
                priority=_as_int(p.get("priority")),
            ),
        )

    if aid == "log_completion":
        completion = item.get_completion()
        completion["completed_at"] = utc_now_naive().isoformat()
        if _as_int(p.get("time_spent_minutes")) is not None:
            completion["time_spent_minutes"] = p["time_spent_minutes"]
        if _as_str(p.get("difficulty")):
            completion["difficulty"] = p["difficulty"]
        if _as_str(p.get("notes")):
            completion["notes"] = p["notes"]
        if _as_str(p.get("blockers")):
            completion["blockers"] = p["blockers"]
        return update_item(
            session,
            item.id,
            ItemUpdate(status="done", completion=completion),
        )

    if aid == "add_subtask":
        step = _as_str(p.get("step"))
        if not step:
            return None
        plan = item.get_plan()
        plan.append({"step": step, "done": bool(p.get("done"))})
        return update_item(session, item.id, ItemUpdate(plan=plan))

    if aid == "break_down_task":
        steps_raw = p.get("steps")
        if not isinstance(steps_raw, list):
            return None
        plan = []
        for s in steps_raw:
            if isinstance(s, dict):
                plan.append({"step": _as_str(s.get("step")) or str(s), "done": bool(s.get("done"))})
            else:
                plan.append({"step": str(s), "done": False})
        return update_item(session, item.id, ItemUpdate(plan=plan))

    return None


def log_item_completion(
    session: Session,
    item_id: int,
    *,
    time_spent_minutes: int | None = None,
    difficulty: str | None = None,
    notes: str | None = None,
    blockers: str | None = None,
) -> ItemOut:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")
    completion = item.get_completion()
    completion["completed_at"] = utc_now_naive().isoformat()
    if time_spent_minutes is not None:
        completion["time_spent_minutes"] = time_spent_minutes
    if difficulty:
        completion["difficulty"] = difficulty
    if notes:
        completion["notes"] = notes
    if blockers:
        completion["blockers"] = blockers
    return update_item(
        session,
        item_id,
        ItemUpdate(status="done", completion=completion),
    )
