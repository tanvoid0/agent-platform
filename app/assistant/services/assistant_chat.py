"""Personal Assistant chat with domain routing, profiles, and planning forms."""

from __future__ import annotations

import json
from typing import Any

import httpx
from fastapi import HTTPException
from sqlmodel import Session, select

from action_orchestrator.engine import build_action_tools, decide_actions
from action_orchestrator.registry import list_actions
from assistant.clarifying_form import (
    build_clarifying_form,
    format_clarifying_answers_message,
    is_clarifying_form,
)
from assistant.domain_forms import domain_for_profile_slug, get_domain_form_spec
from assistant.models import AssistantChatThread
from assistant.services.assistant_router import route_profile_slug
from assistant.services.assistant_service import ensure_assistant_board
from assistant.services.user_profile_service import (
    build_profile_context,
    format_answers_message,
    get_all_profiles,
    get_profile,
    merge_profile,
)
from chat_usage import (
    ContextUsageOut,
    LlmStepUsageOut,
    LlmUsageOut,
    estimate_context_usage,
    merge_llm_usages,
    parse_llm_usage,
)
from context_budget import fit_chat_messages_for_request, max_output_tokens_default
from dag_schema import sanitize_llm_model_alias
from llm_proxy_env import llm_proxy_base_url_v1, llm_proxy_http_timeout_seconds, llm_proxy_master_key
from time_utils import utc_now_naive
from todos.models import PlannerAgentProfile, TodoCategory, TodoItem
from todos.schemas import PlannedActionOut


PA_SYSTEM_PROMPT = """You are the Personal Assistant for a user's daily life planning board.

Your role:
- Understand what the user needs and create organized, actionable plans
- The USER executes tasks — you plan, schedule, and organize
- Check user_domain_profiles and profile_gaps in context BEFORE creating tasks
- When required profile fields are missing, use present_planning_form (prefer domain_form_templates)
- When the user shares personal facts in chat, use store_user_profile to save them
- Use create_item, schedule_item, set_due_date for actionable plans after you have enough context

Never invent body stats, travel dates, or budget numbers — ask via form or chat first."""


def _auto_title_from_message(message: str) -> str:
    text = " ".join(message.split())
    if not text:
        return "New chat"
    if len(text) <= 48:
        return text
    return text[:45] + "..."


def _create_thread_row(
    session: Session,
    project_id: int,
    *,
    title: str | None = "New chat",
) -> AssistantChatThread:
    now = utc_now_naive()
    row = AssistantChatThread(
        project_id=project_id,
        title=title,
        created_at=now,
        updated_at=now,
    )
    row.set_messages([])
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def _get_thread_by_id(
    session: Session, project_id: int, thread_id: int
) -> AssistantChatThread:
    row = session.get(AssistantChatThread, thread_id)
    if not row or row.project_id != project_id:
        raise HTTPException(status_code=404, detail="Chat thread not found")
    return row


def _resolve_thread(
    session: Session, project_id: int, thread_id: int | None = None
) -> AssistantChatThread:
    if thread_id is not None:
        return _get_thread_by_id(session, project_id, thread_id)
    row = session.exec(
        select(AssistantChatThread)
        .where(AssistantChatThread.project_id == project_id)
        .order_by(AssistantChatThread.updated_at.desc())
    ).first()
    if row:
        return row
    return _create_thread_row(session, project_id)


def create_chat_thread(
    session: Session, project_id: int, *, title: str | None = None
) -> AssistantChatThread:
    return _create_thread_row(session, project_id, title=title or "New chat")


def list_chat_threads(session: Session, project_id: int) -> list[dict[str, Any]]:
    rows = session.exec(
        select(AssistantChatThread)
        .where(AssistantChatThread.project_id == project_id)
        .order_by(AssistantChatThread.updated_at.desc())
    ).all()
    out: list[dict[str, Any]] = []
    for row in rows:
        messages = row.get_messages()
        preview = ""
        for m in reversed(messages):
            if m.get("role") == "user" and m.get("content"):
                preview = str(m["content"])[:120]
                break
        out.append(
            {
                "id": row.id,
                "project_id": project_id,
                "title": row.title or "New chat",
                "message_count": len(messages),
                "preview": preview,
                "created_at": row.created_at.isoformat(),
                "updated_at": row.updated_at.isoformat(),
            }
        )
    return out


def _assistant_context_usage(
    *,
    profile: PlannerAgentProfile,
    messages: list[dict[str, Any]],
    board_context: dict[str, Any],
    profile_ctx: dict[str, Any],
    tools: list[dict[str, Any]] | None,
) -> ContextUsageOut:
    system = "\n\n".join(
        p for p in (PA_SYSTEM_PROMPT, profile.system_prompt or "") if p
    )
    injected = json.dumps(
        {**board_context, **profile_ctx}, default=str, ensure_ascii=False
    )
    return estimate_context_usage(
        system_prompt=system,
        tools=tools,
        conversation_messages=messages,
        injected_context=injected,
    )


def _thread_payload(
    session: Session,
    thread: AssistantChatThread,
    project_id: int,
) -> dict[str, Any]:
    board = ensure_assistant_board(session, project_id)
    pending = [PlannedActionOut(**a) for a in thread.get_pending_actions()]
    pending_form = _extract_pending_form(pending)
    profile_slug = thread.last_profile_slug or "personal-assistant"
    profile_ctx = build_profile_context(session, project_id, profile_slug)
    profile = _resolve_profile(session, profile_slug)
    messages = thread.get_messages()
    board_context = build_board_context(session, board.id, project_id)
    tools: list[dict[str, Any]] | None = None
    if profile and profile.action_set_id:
        tools = build_action_tools(list_actions(session, profile.action_set_id))
    context_usage = (
        _assistant_context_usage(
            profile=profile,
            messages=messages,
            board_context=board_context,
            profile_ctx=profile_ctx,
            tools=tools,
        )
        if profile
        else None
    )
    return {
        "thread_id": thread.id,
        "project_id": project_id,
        "board_id": board.id,
        "title": thread.title or "New chat",
        "messages": thread.get_messages(),
        "pending_actions": thread.get_pending_actions(),
        "pending_form": pending_form,
        "last_profile_slug": thread.last_profile_slug,
        "domain_profiles": profile_ctx.get("user_domain_profiles", {}),
        "context_window": context_usage.context_window if context_usage else None,
        "context_usage": context_usage,
    }


def _resolve_profile(session: Session, slug: str) -> PlannerAgentProfile | None:
    row = session.exec(
        select(PlannerAgentProfile).where(PlannerAgentProfile.slug == slug)
    ).first()
    if row:
        return row
    return session.exec(
        select(PlannerAgentProfile).where(PlannerAgentProfile.slug == "personal-assistant")
    ).first()


def build_board_context(session: Session, board_id: int, project_id: int) -> dict[str, Any]:
    from todos.models import TodoBoard

    board = session.get(TodoBoard, board_id)
    categories = session.exec(
        select(TodoCategory).where(TodoCategory.board_id == board_id)
    ).all()
    items = session.exec(
        select(TodoItem).where(TodoItem.board_id == board_id).order_by(TodoItem.updated_at.desc())
    ).all()
    ctx: dict[str, Any] = {
        "board": {
            "id": board_id,
            "name": board.name if board else None,
            "categories": [
                {"id": c.id, "name": c.name, "planner_profile_id": c.planner_profile_id}
                for c in categories
            ],
        },
        "items": [
            {
                "id": i.id,
                "title": i.title,
                "status": i.status,
                "item_kind": i.item_kind,
                "time_horizon": i.time_horizon,
                "due_at": i.due_at.isoformat() if i.due_at else None,
                "scheduled_at": i.scheduled_at.isoformat() if i.scheduled_at else None,
                "category_id": i.category_id,
                "parent_item_id": i.parent_item_id,
            }
            for i in items[:50]
        ],
        "board_id": board_id,
        "categories": [
            {"id": c.id, "name": c.name, "planner_profile_id": c.planner_profile_id}
            for c in categories
        ],
    }
    return ctx


def _form_from_action_params(params: dict[str, Any]) -> dict[str, Any] | None:
    form = params.get("form")
    if not isinstance(form, dict) or not isinstance(form.get("fields"), list):
        return None
    if not form.get("fields"):
        return None
    out = dict(form)
    domain = params.get("domain") or form.get("domain")
    if domain and not out.get("domain"):
        out["domain"] = domain
    return out


def _extract_pending_form(actions: list[PlannedActionOut]) -> dict[str, Any] | None:
    for a in actions:
        if a.action_id == "present_planning_form":
            form = _form_from_action_params(a.parameters or {})
            if form:
                return form
        if a.action_id == "ask_clarifying_questions":
            form = _form_from_action_params(a.parameters or {})
            if form:
                return form
    return None


def _pending_has_interactive_form(actions: list[PlannedActionOut]) -> bool:
    return _extract_pending_form(actions) is not None


def _resolve_form_submit_domain(
    domain: str,
    pending: list[PlannedActionOut],
    *,
    profile_slug: str | None,
) -> str:
    """Prefer explicit domain; else pending form action; else routed profile domain."""
    d = (domain or "").strip().lower()
    if d and d != "general":
        return d
    for a in pending:
        if a.action_id != "present_planning_form":
            continue
        params = a.parameters or {}
        action_domain = params.get("domain")
        if isinstance(action_domain, str) and action_domain.strip():
            return action_domain.strip().lower()
        form = params.get("form")
        if isinstance(form, dict):
            form_domain = form.get("domain")
            if isinstance(form_domain, str) and form_domain.strip():
                return form_domain.strip().lower()
    if profile_slug:
        return domain_for_profile_slug(profile_slug)
    return d or "general"


def _actions_without_forms(actions: list[PlannedActionOut]) -> list[PlannedActionOut]:
    out: list[PlannedActionOut] = []
    for a in actions:
        if a.action_id == "present_planning_form":
            continue
        if a.action_id == "ask_clarifying_questions" and _form_from_action_params(
            a.parameters or {}
        ):
            continue
        out.append(a)
    return out


def _format_conversation_for_planner(
    messages: list[dict[str, Any]],
    *,
    max_turns: int = 12,
) -> list[dict[str, str]]:
    """Compact recent chat turns for the action planner (not action-run history)."""
    turns: list[dict[str, str]] = []
    for m in messages:
        role = m.get("role")
        content = m.get("content")
        if role not in ("user", "assistant") or not content or not str(content).strip():
            continue
        turns.append({"role": str(role), "content": str(content).strip()})
    return turns[-max_turns:]


def _is_form_save_continuation_message(message: str) -> bool:
    """Detect auto-continue summary from submit_planning_form (e.g. 'Saved nutrition profile:')."""
    first = message.strip().split("\n", 1)[0].strip().lower()
    return first.startswith("saved ") and " profile:" in first


def _strip_redundant_profile_saves(
    planned: list[PlannedActionOut],
    message: str,
) -> list[PlannedActionOut]:
    """Drop store_user_profile proposals right after the intake form already saved them."""
    if not _is_form_save_continuation_message(message):
        return planned
    return [a for a in planned if a.action_id != "store_user_profile"]


_INFORMATIONAL_ACTION_IDS = frozenset(
    {"ask_clarifying_questions", "suggest_next_steps", "propose_review"}
)

_ACTION_DISPLAY_NAMES: dict[str, str] = {
    "ask_clarifying_questions": "A few quick questions",
    "suggest_next_steps": "Suggested next steps",
    "present_planning_form": "Details needed",
    "store_user_profile": "Save to your profile",
    "create_item": "Add to your board",
    "propose_review": "Progress review",
}


def _friendly_action_name(action_id: str, name: str | None) -> str:
    n = (name or "").strip()
    if not n or n == action_id:
        return _ACTION_DISPLAY_NAMES.get(
            action_id, action_id.replace("_", " ").title()
        )
    return n


def _questions_from_action(action: PlannedActionOut) -> list[str]:
    p = action.parameters or {}
    qs = p.get("questions")
    if not isinstance(qs, list):
        return []
    return [str(q).strip() for q in qs if q is not None and str(q).strip()]


def _format_questions_in_message(questions: list[str]) -> str:
    if not questions:
        return ""
    if len(questions) == 1:
        return questions[0]
    return "\n".join(f"{i + 1}. {q}" for i, q in enumerate(questions))


def _normalize_planned_actions(
    planned: list[PlannedActionOut],
    *,
    profile: dict[str, Any] | None = None,
) -> list[PlannedActionOut]:
    """Drop invalid clarifying actions and ensure user-facing action names."""
    out: list[PlannedActionOut] = []
    for a in planned:
        name = _friendly_action_name(a.action_id, a.name)
        if a.action_id == "ask_clarifying_questions":
            questions = _questions_from_action(a)
            if not questions:
                continue
            params = dict(a.parameters or {})
            params["questions"] = questions
            llm_fields = params.get("fields")
            if not isinstance(llm_fields, list):
                llm_fields = None
            form = build_clarifying_form(
                questions,
                title=name,
                llm_fields=llm_fields,
                profile=profile,
            )
            if form:
                params["form"] = form
            out.append(
                PlannedActionOut(
                    action_id=a.action_id,
                    name=name,
                    parameters=params,
                    confidence=a.confidence,
                    reasoning=a.reasoning,
                )
            )
            continue
        if name != (a.name or ""):
            out.append(
                PlannedActionOut(
                    action_id=a.action_id,
                    name=name,
                    parameters=a.parameters,
                    confidence=a.confidence,
                    reasoning=a.reasoning,
                )
            )
        else:
            out.append(a)
    return out


def _pending_requires_approval(actions: list[PlannedActionOut]) -> bool:
    task = _actions_without_forms(actions)
    if not task:
        return False
    return any(a.action_id not in _INFORMATIONAL_ACTION_IDS for a in task)


def _pending_is_informational_only(actions: list[PlannedActionOut]) -> bool:
    task = _actions_without_forms(actions)
    return bool(task) and all(a.action_id in _INFORMATIONAL_ACTION_IDS for a in task)


def _thought_is_user_facing(thought: str | None) -> bool:
    """Orchestrator narration (e.g. 'prepared N actions') must not become chat copy."""
    if not thought or not thought.strip():
        return False
    lower = thought.strip().lower()
    if "prepared" in lower and "action" in lower:
        return False
    if "for your review" in lower:
        return False
    return True


def _resolve_pending_proposal_in_messages(
    messages: list[dict[str, Any]], status: str
) -> None:
    for i in range(len(messages) - 1, -1, -1):
        m = messages[i]
        if m.get("role") == "assistant" and m.get("proposal_status") == "pending":
            m["proposal_status"] = status
            return


def _assistant_message_with_usage(
    content: str,
    usage_steps: list[LlmStepUsageOut],
    *,
    proposed_actions: list[PlannedActionOut] | None = None,
) -> dict[str, Any]:
    msg: dict[str, Any] = {"role": "assistant", "content": content}
    turn = merge_llm_usages(usage_steps)
    if turn.total_tokens or turn.cost_usd:
        msg["usage"] = {
            "prompt_tokens": turn.prompt_tokens,
            "completion_tokens": turn.completion_tokens,
            "total_tokens": turn.total_tokens,
            "cost_usd": turn.cost_usd,
        }
    if proposed_actions:
        msg["proposed_actions"] = [p.model_dump() for p in proposed_actions]
        msg["proposal_status"] = "pending"
    return msg


def _append_assistant_message(
    messages: list[dict[str, Any]],
    content: str,
    *,
    proposed_actions: list[PlannedActionOut] | None = None,
) -> None:
    msg: dict[str, Any] = {"role": "assistant", "content": content}
    if proposed_actions:
        msg["proposed_actions"] = [p.model_dump() for p in proposed_actions]
        msg["proposal_status"] = "pending"
    messages.append(msg)


def resolve_thread_proposal(thread: AssistantChatThread, status: str) -> None:
    """Mark the latest pending proposal snapshot on the thread (approved / dismissed)."""
    messages = thread.get_messages()
    _resolve_pending_proposal_in_messages(messages, status)
    thread.set_messages(messages)


def _assistant_reply_for_actions(
    task_actions: list[PlannedActionOut],
    *,
    thought: str | None = None,
) -> str:
    if not task_actions:
        return "Let me know if you'd like me to break this down into tasks."
    if len(task_actions) == 1:
        a = task_actions[0]
        p = a.parameters or {}
        aid = a.action_id
        if aid == "ask_clarifying_questions":
            if _form_from_action_params(p):
                return (
                    "I need a few details before I can put together your plan — "
                    "use the form below (yes/no, picks, or short answers)."
                )
            questions = _questions_from_action(a)
            if questions:
                block = _format_questions_in_message(questions)
                return (
                    "I need a few details before I can put together your plan:\n\n"
                    f"{block}\n\n"
                    "Reply in chat with whatever you know — even partial answers help."
                )
            return (
                "I need a few more details before I can put together your plan — "
                "share anything relevant in your next message."
            )
        if aid == "suggest_next_steps":
            guidance = p.get("guidance")
            if isinstance(guidance, str) and guidance.strip():
                return guidance.strip()
            return "Here are some suggested next steps."
        if aid == "create_item":
            title = p.get("title")
            if isinstance(title, str) and title.strip():
                return f'I can add "{title.strip()}" to your board — confirm below when it looks right.'
        if aid == "create_habit":
            title = p.get("title")
            if isinstance(title, str) and title.strip():
                return f'I can track "{title.strip()}" as a habit on your board.'
        if aid == "break_down_task":
            steps = p.get("steps")
            if isinstance(steps, list) and steps:
                return "I've broken this into steps — review them below and add to your board if you'd like."
            guidance = p.get("guidance")
            if isinstance(guidance, str) and guidance.strip():
                return guidance.strip()
            return "Here's a step-by-step plan — review below and add what you'd like to your board."
        if aid == "store_user_profile":
            return "Got it — I'll use that for planning. Tell me what you'd like to tackle next."
        if aid == "propose_review":
            reason = p.get("reason")
            if isinstance(reason, str) and reason.strip():
                return reason.strip()
        if _thought_is_user_facing(thought):
            return thought.strip()
    if all(a.action_id in _INFORMATIONAL_ACTION_IDS for a in task_actions):
        return "A few things to look at below — reply in chat when you're ready."
    creates = sum(1 for a in task_actions if a.action_id in ("create_item", "create_habit"))
    if creates == len(task_actions):
        return (
            f"I've lined up {creates} item{'s' if creates != 1 else ''} for your board — "
            "confirm below to add them."
        )
    n = len(task_actions)
    return f"I have {n} suggestion{'s' if n != 1 else ''} for your board — take a look below."


def _maybe_inject_domain_form(
    profile_slug: str,
    profile_ctx: dict[str, Any],
    planned: list[PlannedActionOut],
) -> list[PlannedActionOut]:
    """If profile gaps exist and LLM didn't ask, inject standard domain form."""
    if any(a.action_id == "present_planning_form" for a in planned):
        return planned
    gaps = profile_ctx.get("active_profile_gaps") or []
    domain = profile_ctx.get("active_domain")
    if not gaps or not domain:
        return planned
    spec = get_domain_form_spec(domain)
    if not spec:
        return planned
    return [
        PlannedActionOut(
            action_id="present_planning_form",
            name="Present planning form",
            parameters={"domain": domain, "form": spec},
            confidence=1.0,
            reasoning="Required profile fields missing — showing intake form.",
        ),
        *planned,
    ]


def format_apply_summary(result: Any) -> str:
    """Render a board_action_apply result as a synthetic user turn for auto-continue."""
    parts: list[str] = []
    if result.applied:
        parts.append("Applied: " + "; ".join(result.applied) + ".")
    if result.skipped:
        parts.append("Skipped: " + "; ".join(result.skipped) + ".")
    if result.guidance:
        parts.append(" ".join(result.guidance))
    if not parts:
        return "Done."
    return " ".join(parts)


async def get_thread(
    session: Session, project_id: int, *, thread_id: int | None = None
) -> dict[str, Any]:
    thread = _resolve_thread(session, project_id, thread_id)
    return _thread_payload(session, thread, project_id)


def get_context_usage(
    session: Session, project_id: int, *, thread_id: int | None = None
) -> ContextUsageOut:
    thread = _resolve_thread(session, project_id, thread_id)
    board = ensure_assistant_board(session, project_id)
    profile_slug = thread.last_profile_slug or "personal-assistant"
    profile = _resolve_profile(session, profile_slug)
    if not profile:
        raise HTTPException(status_code=400, detail="No planner profile configured")
    profile_ctx = build_profile_context(session, project_id, profile_slug)
    board_context = build_board_context(session, board.id, project_id)
    tools: list[dict[str, Any]] | None = None
    if profile.action_set_id:
        tools = build_action_tools(list_actions(session, profile.action_set_id))
    return _assistant_context_usage(
        profile=profile,
        messages=thread.get_messages(),
        board_context=board_context,
        profile_ctx=profile_ctx,
        tools=tools,
    )


async def _generate_assistant_turn(
    session: Session,
    project_id: int,
    thread: AssistantChatThread,
    message: str,
    *,
    model: str | None = None,
    delegate_slug: str | None = None,
    propose_actions: bool = True,
) -> dict[str, Any]:
    """Regenerate assistant reply; thread messages must end with this user message."""
    board = ensure_assistant_board(session, project_id)
    profile_slug = route_profile_slug(message, explicit=delegate_slug)
    profile = _resolve_profile(session, profile_slug)
    if not profile:
        raise HTTPException(status_code=400, detail="No planner profile configured")

    messages = thread.get_messages()
    llm_model = model or profile.default_model or board.default_model or "gemma4:31b-cloud"
    content = ""
    planned_out: list[PlannedActionOut] = []
    thought: str | None = None
    pending_form: dict[str, Any] | None = None

    profile_ctx = build_profile_context(session, project_id, profile_slug)
    board_context = build_board_context(session, board.id, project_id)
    actions = list_actions(session, profile.action_set_id) if profile.action_set_id else []
    tools = build_action_tools(actions) if actions else None
    context_usage = _assistant_context_usage(
        profile=profile,
        messages=messages,
        board_context=board_context,
        profile_ctx=profile_ctx,
        tools=tools,
    )
    usage_steps: list[LlmStepUsageOut] = []

    if propose_actions and profile.action_set_id:
        context = dict(board_context)
        context.update(profile_ctx)
        context["planner_system_prompt"] = profile.system_prompt
        context["personal_assistant_prompt"] = PA_SYSTEM_PROMPT
        context["conversation_history"] = _format_conversation_for_planner(messages[:-1])

        planned, thought, decide_usage = await decide_actions(
            goal=message,
            context=context,
            actions=actions,
            history=None,
            llm_model=llm_model,
        )
        usage_steps.extend(decide_usage.steps)
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
        planned_out = _strip_redundant_profile_saves(planned_out, message)
        planned_out = _normalize_planned_actions(
            planned_out,
            profile=profile_ctx.get("active_profile"),
        )
        planned_out = _maybe_inject_domain_form(profile_slug, profile_ctx, planned_out)
        pending_form = _extract_pending_form(planned_out)
        task_actions = _actions_without_forms(planned_out)
        needs_approval = _pending_requires_approval(planned_out)

        if pending_form and not task_actions:
            clarify_actions = [
                a for a in planned_out if a.action_id == "ask_clarifying_questions"
            ]
            if clarify_actions and is_clarifying_form(pending_form):
                content = _assistant_reply_for_actions(clarify_actions, thought=thought)
            else:
                content = (
                    pending_form.get("description")
                    or "Please fill in a few details so I can plan this properly."
                )
                if _thought_is_user_facing(thought):
                    content = thought.strip()
        elif task_actions:
            content = _assistant_reply_for_actions(task_actions, thought=thought)
        elif _thought_is_user_facing(thought):
            content = thought.strip()
        else:
            content, chat_usage = await _chat_only(
                session,
                profile,
                message,
                messages,
                llm_model,
                board.id,
                project_id,
                profile_slug,
            )
            usage_steps.extend(chat_usage.steps)
            if not (content or "").strip():
                content = (
                    "Tell me a bit more about what you want on your board — "
                    "for example meals for the week, prep day, or dietary constraints."
                )

        persist_pending = needs_approval or _pending_has_interactive_form(
            planned_out
        )
        if persist_pending:
            thread.set_pending_actions([p.model_dump() for p in planned_out])
            messages.append(
                _assistant_message_with_usage(
                    content,
                    usage_steps,
                    proposed_actions=planned_out if planned_out else None,
                )
            )
        else:
            thread.set_pending_actions([])
            messages.append(_assistant_message_with_usage(content, usage_steps))
    else:
        content, chat_usage = await _chat_only(
            session, profile, message, messages, llm_model, board.id, project_id, profile_slug
        )
        usage_steps.extend(chat_usage.steps)
        messages.append(_assistant_message_with_usage(content, usage_steps))

    turn_usage = merge_llm_usages(usage_steps)

    thread.set_messages(messages)
    thread.last_profile_slug = profile_slug
    thread.updated_at = utc_now_naive()
    session.add(thread)
    session.commit()
    session.refresh(thread)

    pending = thread.get_pending_actions()
    if pending_form is None:
        pending_form = _extract_pending_form([PlannedActionOut(**a) for a in pending])

    return {
        "thread_id": thread.id,
        "content": content,
        "model": llm_model,
        "profile_slug": profile_slug,
        "thought": thought,
        "actions": pending,
        "messages": thread.get_messages(),
        "pending_actions": pending,
        "pending_form": pending_form,
        "board_id": board.id,
        "domain_profiles": profile_ctx.get("user_domain_profiles", {}),
        "context_window": context_usage.context_window,
        "context_usage": context_usage,
        "usage": turn_usage,
    }


async def send_chat_message(
    session: Session,
    project_id: int,
    message: str,
    *,
    thread_id: int | None = None,
    model: str | None = None,
    delegate_slug: str | None = None,
    propose_actions: bool = True,
) -> dict[str, Any]:
    thread = _resolve_thread(session, project_id, thread_id)
    if not thread.title or thread.title == "New chat":
        thread.title = _auto_title_from_message(message)

    messages = thread.get_messages()
    _resolve_pending_proposal_in_messages(messages, "superseded")
    stale_pending = [PlannedActionOut(**a) for a in thread.get_pending_actions()]
    if _pending_is_informational_only(stale_pending):
        thread.set_pending_actions([])
    messages.append({"role": "user", "content": message})
    thread.set_messages(messages)
    # Commit the user turn before generation: an LLM failure or client disconnect
    # must not lose it — the thread can then be retried/regenerated as-is.
    thread.updated_at = utc_now_naive()
    session.add(thread)
    session.commit()

    return await _generate_assistant_turn(
        session,
        project_id,
        thread,
        message,
        model=model,
        delegate_slug=delegate_slug,
        propose_actions=propose_actions,
    )


async def retry_chat_message(
    session: Session,
    project_id: int,
    thread_id: int,
    message_index: int,
    *,
    model: str | None = None,
    propose_actions: bool = True,
) -> dict[str, Any]:
    """Drop messages after message_index and regenerate the assistant reply."""
    thread = _get_thread_by_id(session, project_id, thread_id)
    messages = thread.get_messages()
    if message_index < 0 or message_index >= len(messages):
        raise HTTPException(status_code=400, detail="message_index out of range")
    target = messages[message_index]
    if target.get("role") != "user":
        raise HTTPException(
            status_code=400,
            detail="message_index must point to a user message",
        )
    message = str(target.get("content") or "").strip()
    if not message:
        raise HTTPException(status_code=400, detail="User message is empty")

    truncated = messages[: message_index + 1]
    _resolve_pending_proposal_in_messages(truncated, "superseded")
    thread.set_messages(truncated)
    thread.set_pending_actions([])
    session.add(thread)
    session.commit()
    session.refresh(thread)

    return await _generate_assistant_turn(
        session,
        project_id,
        thread,
        message,
        model=model,
        delegate_slug=thread.last_profile_slug,
        propose_actions=propose_actions,
    )


async def submit_planning_form(
    session: Session,
    project_id: int,
    *,
    domain: str,
    answers: dict[str, Any],
    thread_id: int | None = None,
    auto_continue: bool = True,
    model: str | None = None,
) -> dict[str, Any]:
    """Save profile form answers or submit clarifying Q&A and optionally continue."""
    thread = _resolve_thread(session, project_id, thread_id)
    pending_raw = thread.get_pending_actions()
    pending = [PlannedActionOut(**a) for a in pending_raw]
    pending_form = _extract_pending_form(pending)

    if pending_form and is_clarifying_form(pending_form):
        remaining = [
            a
            for a in pending
            if a.action_id != "ask_clarifying_questions"
        ]
        thread.set_pending_actions([a.model_dump() for a in remaining])
        summary = format_clarifying_answers_message(pending_form, answers)
        messages = thread.get_messages()
        messages.append({"role": "user", "content": summary})
        thread.set_messages(messages)
        thread.updated_at = utc_now_naive()
        session.add(thread)
        session.commit()

        if not auto_continue:
            return {
                "thread_id": thread.id,
                "messages": thread.get_messages(),
                "pending_actions": thread.get_pending_actions(),
                "pending_form": _extract_pending_form(
                    [PlannedActionOut(**a) for a in thread.get_pending_actions()]
                ),
            }

        return await send_chat_message(
            session,
            project_id,
            summary,
            thread_id=thread.id,
            model=model,
            delegate_slug=thread.last_profile_slug,
            propose_actions=True,
        )

    domain = _resolve_form_submit_domain(
        domain,
        pending,
        profile_slug=thread.last_profile_slug,
    )
    merge_profile(session, project_id, domain, answers)
    remaining = _actions_without_forms(pending)
    thread.set_pending_actions([a.model_dump() for a in remaining])

    summary = format_answers_message(domain, answers)

    if not auto_continue:
        messages = thread.get_messages()
        messages.append({"role": "user", "content": summary})
        thread.set_messages(messages)
        thread.updated_at = utc_now_naive()
        session.add(thread)
        session.commit()
        return {
            "thread_id": thread.id,
            "saved_domain": domain,
            "profile": get_profile(session, project_id, domain),
            "messages": thread.get_messages(),
            "pending_actions": thread.get_pending_actions(),
        }

    return await send_chat_message(
        session,
        project_id,
        summary,
        thread_id=thread.id,
        model=model,
        delegate_slug=thread.last_profile_slug,
        propose_actions=True,
    )


async def _chat_only(
    session: Session,
    profile: PlannerAgentProfile,
    message: str,
    history: list[dict[str, str]],
    model: str,
    board_id: int,
    project_id: int,
    profile_slug: str,
) -> tuple[str, LlmUsageOut]:
    profile_ctx = build_profile_context(session, project_id, profile_slug)
    context = build_board_context(session, board_id, project_id)
    context.update(profile_ctx)
    system_parts = [PA_SYSTEM_PROMPT, profile.system_prompt, f"Context:\n{context}"]
    messages: list[dict[str, str]] = [{"role": "system", "content": "\n\n".join(system_parts)}]
    for h in history[:-1]:
        if h.get("content"):
            messages.append({"role": h.get("role", "user"), "content": h["content"]})
    messages.append({"role": "user", "content": message})

    key = llm_proxy_master_key()
    if not key:
        raise HTTPException(status_code=503, detail="AGENT_PLATFORM_MASTER_KEY is not set.")

    fitted, _ = fit_chat_messages_for_request(messages)
    payload: dict[str, Any] = {
        "messages": fitted,
        "max_tokens": max_output_tokens_default(),
    }
    sm = sanitize_llm_model_alias(model) if model else None
    if sm:
        payload["model"] = sm

    base = llm_proxy_base_url_v1()
    headers = {"Content-Type": "application/json", "Authorization": f"Bearer {key}"}
    try:
        async with httpx.AsyncClient(timeout=llm_proxy_http_timeout_seconds()) as client:
            r = await client.post(f"{base}/chat/completions", headers=headers, json=payload)
    except httpx.RequestError as e:
        raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e

    if r.status_code != 200:
        raise HTTPException(status_code=502, detail=f"LLM proxy returned HTTP {r.status_code}")

    data = r.json()
    usage = merge_llm_usages([parse_llm_usage(data, label="chat_only")])
    choices = data.get("choices") or []
    if choices:
        content = (choices[0].get("message") or {}).get("content") or ""
        return content, usage
    return "", usage
