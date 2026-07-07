"""Bridge todo items to action orchestrator and chat."""

from __future__ import annotations

from typing import Any

import httpx
from fastapi import HTTPException
from sqlmodel import Session

from action_orchestrator.engine import decide_actions
from action_orchestrator.registry import list_actions
from context_budget import fit_chat_messages_for_request, max_output_tokens_default
from dag_schema import sanitize_llm_model_alias
from llm_proxy_env import llm_proxy_base_url_v1, llm_proxy_http_timeout_seconds, llm_proxy_master_key
from document_service import read_workspace_file_for_llm
from todos.models import TodoBoard, TodoCategory, TodoItem
from workspace_service import WorkspaceError
from todos.schemas import AgentChatResponse, AgentStepResponse, PlannedActionOut
from todos.services.board_service import resolve_profile_for_item
from todos.services.item_events import append_item_event


def build_item_context(session: Session, item: TodoItem) -> dict[str, Any]:
    board = session.get(TodoBoard, item.board_id)
    category = session.get(TodoCategory, item.category_id) if item.category_id else None
    profile = resolve_profile_for_item(session, item)
    return {
        "item": {
            "id": item.id,
            "title": item.title,
            "description": item.description,
            "status": item.status,
            "priority": item.priority,
            "tags": item.get_tags(),
            "plan": item.get_plan(),
            "metadata": item.get_metadata(),
        },
        "board": {
            "id": board.id if board else item.board_id,
            "name": board.name if board else None,
            "default_model": board.default_model if board else None,
        },
        "category": {
            "id": category.id,
            "name": category.name,
            "planner_profile_id": category.planner_profile_id,
        }
        if category
        else None,
        "planner_profile": {
            "slug": profile.slug,
            "name": profile.name,
            "requirement_type": profile.requirement_type,
        }
        if profile
        else None,
    }


def resolve_model(
    session: Session,
    item: TodoItem,
    override: str | None,
) -> str | None:
    if override and override.strip():
        return sanitize_llm_model_alias(override.strip()) or override.strip()
    profile = resolve_profile_for_item(session, item)
    if profile and profile.default_model:
        return profile.default_model
    board = session.get(TodoBoard, item.board_id)
    if board and board.default_model:
        return board.default_model
    return "gemma4:31b-cloud"


def merge_workspace_documents(
    session: Session,
    item: TodoItem,
    context: dict[str, Any],
) -> None:
    """Attach workspace document excerpts when context includes document_paths."""
    paths = context.get("document_paths")
    if paths is None and context.get("document_path"):
        paths = [context["document_path"]]
    if not isinstance(paths, list) or not paths:
        return

    board = session.get(TodoBoard, item.board_id)
    project_id = board.project_id if board else None
    if not project_id:
        return

    docs: list[dict[str, Any]] = []
    for raw in paths:
        if not isinstance(raw, str) or not raw.strip():
            continue
        rel = raw.strip()
        try:
            payload = read_workspace_file_for_llm(project_id, rel)
            content = payload.get("content") or ""
            if len(content) > 8000:
                content = content[:8000] + "\n\n_(truncated)_"
            docs.append(
                {
                    "path": payload.get("path", rel),
                    "content_kind": payload.get("content_kind"),
                    "excerpt": content,
                }
            )
        except WorkspaceError as e:
            docs.append({"path": rel, "error": getattr(e, "code", str(e))})

    if docs:
        context["workspace_documents"] = docs


async def agent_step(
    session: Session,
    item_id: int,
    goal: str,
    model: str | None,
    extra_context: dict[str, Any],
) -> AgentStepResponse:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")

    profile = resolve_profile_for_item(session, item)
    if not profile or not profile.action_set_id:
        raise HTTPException(status_code=400, detail="No planner profile or action set configured")

    actions = list_actions(session, profile.action_set_id)
    if not actions:
        raise HTTPException(status_code=400, detail="Action set has no actions")

    context = build_item_context(session, item)
    context.update(extra_context)
    merge_workspace_documents(session, item, context)
    if profile.system_prompt:
        context["planner_system_prompt"] = profile.system_prompt

    llm_model = resolve_model(session, item, model)
    planned, thought, _ = await decide_actions(
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

    append_item_event(
        session,
        item_id,
        "agent_step",
        {
            "goal": goal,
            "thought": thought,
            "actions": [p.model_dump() for p in planned_out],
            "profile_slug": profile.slug,
        },
    )

    return AgentStepResponse(
        thought=thought,
        actions=planned_out,
        profile_slug=profile.slug,
        action_set_id=profile.action_set_id,
    )


async def agent_chat(
    session: Session,
    item_id: int,
    message: str,
    model: str | None,
    history: list[dict[str, str]],
) -> AgentChatResponse:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")

    profile = resolve_profile_for_item(session, item)
    context = build_item_context(session, item)
    llm_model = resolve_model(session, item, model)

    system_parts = [
        "You are a helpful planning assistant for a personal todo board.",
        "Guide the user on what to do next. Be concise and actionable.",
    ]
    if profile:
        system_parts.append(profile.system_prompt)
    system_parts.append(f"Current task context:\n{context}")

    messages: list[dict[str, str]] = [{"role": "system", "content": "\n\n".join(system_parts)}]
    for h in history:
        role = h.get("role", "user")
        content = h.get("content", "")
        if content:
            messages.append({"role": role, "content": content})
    messages.append({"role": "user", "content": message})

    key = llm_proxy_master_key()
    if not key:
        raise HTTPException(status_code=503, detail="AGENT_PLATFORM_MASTER_KEY is not set.")

    fitted, _ = fit_chat_messages_for_request(messages)
    payload: dict[str, Any] = {
        "messages": fitted,
        "max_tokens": max_output_tokens_default(),
    }
    sm = sanitize_llm_model_alias(llm_model) if llm_model else None
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
    content = ""
    choices = data.get("choices") or []
    if choices:
        content = (choices[0].get("message") or {}).get("content") or ""

    append_item_event(
        session,
        item_id,
        "agent_chat",
        {"message": message, "model": llm_model, "profile_slug": profile.slug if profile else None},
    )

    return AgentChatResponse(
        content=content,
        model=llm_model,
        profile_slug=profile.slug if profile else None,
    )
