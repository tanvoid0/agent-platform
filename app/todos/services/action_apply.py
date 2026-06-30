"""Apply agent-planned actions on the server (single source of truth)."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from fastapi import HTTPException
from sqlmodel import Session

from time_utils import utc_now_naive
from todos.models import TODO_STATUSES, TodoItem
from todos.schemas import ItemCreate, ItemOut, ItemUpdate, PlannedActionOut
from todos.services.board_service import _item_out, create_item, update_item
from todos.services.item_events import append_item_event
from todos.services.server_actions import execute_trigger_webhook


class ApplyResult:
    def __init__(self) -> None:
        self.applied: list[str] = []
        self.skipped: list[str] = []
        self.guidance: list[str] = []
        self.exports: list[dict[str, Any]] = []


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


def apply_planned_actions(
    session: Session,
    item_id: int,
    actions: list[PlannedActionOut],
) -> tuple[ItemOut, ApplyResult]:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")

    result = ApplyResult()
    for action in actions:
        p = action.parameters or {}
        target_id = _as_int(p.get("item_id")) or item_id
        if target_id != item_id:
            result.skipped.append(f"{action.action_id}: wrong item_id")
            continue

        aid = action.action_id
        if aid == "move_item_status":
            status = _as_str(p.get("status"))
            if not status or status not in TODO_STATUSES:
                result.skipped.append("move_item_status: invalid status")
                continue
            item.status = status
            result.applied.append(f"Moved to {status}")

        elif aid == "update_item":
            title = _as_str(p.get("title"))
            description = p.get("description")
            priority = _as_int(p.get("priority"))
            changed = False
            if title:
                item.title = title.strip()
                changed = True
            if isinstance(description, str):
                item.description = description
                changed = True
            if priority is not None:
                item.priority = priority
                changed = True
            if not changed:
                result.skipped.append("update_item: empty patch")
                continue
            result.applied.append("Updated item")

        elif aid == "add_subtask":
            step = _as_str(p.get("step"))
            if not step:
                result.skipped.append("add_subtask: missing step")
                continue
            plan = item.get_plan()
            plan.append({"step": step, "done": bool(p.get("done"))})
            item.set_plan(plan)
            result.applied.append(f"Added subtask: {step}")

        elif aid == "break_down_task":
            grocery = p.get("grocery_groups")
            if isinstance(grocery, list) and grocery:
                plan = []
                for g in grocery:
                    if not isinstance(g, dict):
                        continue
                    category = _as_str(g.get("category")) or "Other"
                    items_raw = g.get("items")
                    items = (
                        [str(x) for x in items_raw]
                        if isinstance(items_raw, list)
                        else []
                    )
                    plan.append({"category": category, "items": items, "done": False})
                if not plan:
                    result.skipped.append("break_down_task: empty grocery_groups")
                    continue
                item.set_plan(plan)
                meta = item.get_metadata()
                meta["plan_kind"] = "grocery_list"
                item.set_metadata(meta)
                result.applied.append(f"Grocery list: {len(plan)} groups")
            else:
                steps_raw = p.get("steps")
                if not isinstance(steps_raw, list) or not steps_raw:
                    result.skipped.append("break_down_task: no steps")
                    continue
                plan = []
                for s in steps_raw:
                    if isinstance(s, dict):
                        step = _as_str(s.get("step")) or str(s)
                        plan.append({"step": step, "done": bool(s.get("done"))})
                    else:
                        plan.append({"step": str(s), "done": False})
                item.set_plan(plan)
                result.applied.append(f"Plan: {len(plan)} steps")

        elif aid == "suggest_next_steps":
            guidance = _as_str(p.get("guidance"))
            if guidance:
                result.guidance.append(guidance)
            result.applied.append("Guidance received")

        elif aid == "ask_clarifying_questions":
            qs = p.get("questions")
            if isinstance(qs, list):
                result.guidance.extend(str(q) for q in qs)
            result.applied.append("Questions received")

        elif aid == "present_planning_form":
            form = p.get("form")
            if not isinstance(form, dict):
                result.skipped.append("present_planning_form: invalid form")
                continue
            meta = item.get_metadata()
            forms = meta.get("planning_forms")
            if not isinstance(forms, list):
                forms = []
            forms.append({"spec": form, "status": "open", "answers": None})
            meta["planning_forms"] = forms
            meta["pending_form_index"] = len(forms) - 1
            item.set_metadata(meta)
            result.applied.append("Planning form presented")

        elif aid == "export_markdown_checklist":
            title = _as_str(p.get("title")) or item.title
            lines_raw = p.get("lines")
            lines: list[str] = []
            if isinstance(lines_raw, list):
                lines = [str(x) for x in lines_raw]
            elif item.get_plan():
                lines = [s.get("step", "") for s in item.get_plan()]
            body = "\n".join(f"- [ ] {line}" for line in lines if line.strip())
            content = f"# {title}\n\n{body}\n"
            filename = f"{title[:48].strip() or 'checklist'}.md"
            result.exports.append({"kind": "markdown", "filename": filename, "content": content})
            result.applied.append("Markdown checklist ready")

        elif aid == "export_ics_event":
            summary = _as_str(p.get("summary")) or item.title
            start = _as_str(p.get("start")) or ""
            end = _as_str(p.get("end")) or start
            description = _as_str(p.get("description")) or item.description or ""
            if not start:
                result.skipped.append("export_ics_event: missing start")
                continue
            uid = f"todo-item-{item.id}@agent-platform"
            ics_description = description.replace("\n", "\\n")
            ics = (
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Agent Platform//Todo Planner//EN\r\n"
                "BEGIN:VEVENT\r\n"
                f"UID:{uid}\r\n"
                f"SUMMARY:{summary}\r\n"
                f"DTSTART:{start}\r\n"
                f"DTEND:{end or start}\r\n"
                f"DESCRIPTION:{ics_description}\r\n"
                "END:VEVENT\r\nEND:VCALENDAR\r\n"
            )
            result.exports.append(
                {
                    "kind": "ics",
                    "filename": f"{summary[:48].strip() or 'event'}.ics",
                    "content": ics,
                }
            )
            result.applied.append("Calendar event ready")

        elif aid == "schedule_item":
            scheduled = _parse_dt(p.get("scheduled_at"))
            if not scheduled:
                result.skipped.append("schedule_item: invalid scheduled_at")
                continue
            item.scheduled_at = scheduled
            horizon = _as_str(p.get("time_horizon"))
            if horizon:
                item.time_horizon = horizon
            result.applied.append("Scheduled item")

        elif aid == "set_due_date":
            due = _parse_dt(p.get("due_at"))
            if not due:
                result.skipped.append("set_due_date: invalid due_at")
                continue
            item.due_at = due
            result.applied.append("Due date set")

        elif aid == "log_completion":
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
            item.set_completion(completion)
            item.status = "done"
            result.applied.append("Completion logged")

        elif aid == "adjust_plan":
            title = _as_str(p.get("title"))
            if title:
                item.title = title.strip()
            if isinstance(p.get("description"), str):
                item.description = p["description"]
            due = _parse_dt(p.get("due_at"))
            if due:
                item.due_at = due
            scheduled = _parse_dt(p.get("scheduled_at"))
            if scheduled:
                item.scheduled_at = scheduled
            horizon = _as_str(p.get("time_horizon"))
            if horizon:
                item.time_horizon = horizon
            status = _as_str(p.get("status"))
            if status and status in TODO_STATUSES:
                item.status = status
            priority = _as_int(p.get("priority"))
            if priority is not None:
                item.priority = priority
            result.applied.append("Plan adjusted")

        elif aid == "create_subtask_item":
            parent_id = _as_int(p.get("parent_item_id")) or item_id
            title = _as_str(p.get("title"))
            if not title:
                result.skipped.append("create_subtask_item: missing title")
                continue
            parent = session.get(TodoItem, parent_id)
            if not parent:
                result.skipped.append("create_subtask_item: parent not found")
                continue
            sub = create_item(
                session,
                parent.board_id,
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
            result.applied.append(f"Created subtask: {sub.title}")

        elif aid == "propose_review":
            reason = _as_str(p.get("reason")) or "Review suggested"
            result.guidance.append(reason)
            result.applied.append("Review proposed")

        elif aid == "store_user_profile":
            domain = _as_str(p.get("domain"))
            data = p.get("data")
            if not domain or not isinstance(data, dict):
                result.skipped.append("store_user_profile: invalid domain or data")
                continue
            from todos.models import TodoBoard
            from assistant.services.user_profile_service import merge_profile

            board_row = session.get(TodoBoard, item.board_id)
            if not board_row or board_row.project_id is None:
                result.skipped.append("store_user_profile: no project")
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
            except ValueError as e:
                result.skipped.append(f"trigger_webhook: {e}")
            except Exception as e:
                result.skipped.append(f"trigger_webhook: {e}")

        else:
            result.skipped.append(f"Unknown action: {aid}")

    item.updated_at = utc_now_naive()
    session.add(item)
    session.commit()
    session.refresh(item)

    append_item_event(
        session,
        item_id,
        "actions_applied",
        {
            "applied": result.applied,
            "skipped": result.skipped,
            "guidance": result.guidance,
            "export_count": len(result.exports),
            "actions": [a.model_dump() for a in actions],
        },
    )

    return _item_out(item), result


def submit_planning_form(
    session: Session,
    item_id: int,
    form_index: int,
    answers: dict[str, Any],
) -> ItemOut:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")

    meta = item.get_metadata()
    forms = meta.get("planning_forms")
    if not isinstance(forms, list) or form_index < 0 or form_index >= len(forms):
        raise HTTPException(status_code=400, detail="Invalid planning form index")

    entry = forms[form_index]
    if not isinstance(entry, dict):
        raise HTTPException(status_code=400, detail="Invalid planning form entry")

    entry["answers"] = answers
    entry["status"] = "submitted"
    meta["planning_forms"] = forms
    if meta.get("pending_form_index") == form_index:
        meta.pop("pending_form_index", None)
    item.set_metadata(meta)
    item.updated_at = utc_now_naive()
    session.add(item)
    session.commit()
    session.refresh(item)

    append_item_event(
        session,
        item_id,
        "planning_form_submitted",
        {"form_index": form_index, "answers": answers},
    )
    return _item_out(item)
