"""Coder agent REST routes — workspace-bound coding assistant."""

from __future__ import annotations

import json

from fastapi import APIRouter, Depends, HTTPException, Query
from fastapi.responses import StreamingResponse
from sqlmodel import Session

from api_tokens.auth import TokenPrincipal, require_scope, require_valid_token
from api_tokens.usage_tracking import record_api_token_usage
from chat_usage import ContextUsageOut
from coder.schemas import (
    CoderApprovalRequest,
    CoderChatSendRequest,
    CoderChatSendResponse,
    CoderThreadCreateOut,
    CoderThreadCreateRequest,
    CoderThreadDeleteOut,
    CoderThreadOut,
    CoderThreadsListOut,
)
from coder.service import (
    create_thread,
    delete_thread,
    get_context_usage,
    get_thread,
    list_threads,
    resolve_pending_call,
    send_message,
    stream_message,
)
from database import get_session
from llm_proxy_env import llm_proxy_master_key

router = APIRouter(prefix="/coder", tags=["coder"])


def _record_usage_from_done(session: Session, token_id: int | None, chunk: str) -> None:
    if not chunk.startswith("event: done\n"):
        return
    for line in chunk.split("\n"):
        if line.startswith("data:"):
            try:
                payload = json.loads(line[len("data:") :].strip())
            except json.JSONDecodeError:
                return
            usage = payload.get("usage") or {}
            record_api_token_usage(
                session,
                token_id,
                tokens=int(usage.get("total_tokens") or 0),
                cost=float(usage.get("cost_usd") or 0.0),
            )
            return


async def _usage_tracking_stream(gen, session: Session, token_id: int | None):
    async for chunk in gen:
        _record_usage_from_done(session, token_id, chunk)
        yield chunk


@router.get("/chat/threads", response_model=CoderThreadsListOut)
def coder_threads_list(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "chat:write")
    return CoderThreadsListOut(threads=list_threads(session))


@router.post("/chat/threads", response_model=CoderThreadCreateOut)
def coder_threads_create(
    body: CoderThreadCreateRequest = ...,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "chat:write")
    row = create_thread(session, title=body.title, workspace_root=body.workspace_root)
    return CoderThreadCreateOut(
        thread_id=row.id,
        title=row.title or "New session",
        workspace_root=row.workspace_root,
    )


@router.get("/chat/context-usage", response_model=ContextUsageOut)
def coder_context_usage(
    thread_id: int | None = Query(default=None, ge=1),
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "chat:write")
    return get_context_usage(session, thread_id=thread_id)


@router.get("/chat/thread", response_model=CoderThreadOut)
async def coder_thread(
    thread_id: int | None = Query(default=None, ge=1),
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "chat:write")
    data = await get_thread(session, thread_id=thread_id)
    return CoderThreadOut(**data)


@router.delete("/chat/thread/{thread_id}", response_model=CoderThreadDeleteOut)
def coder_thread_delete(
    thread_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "chat:write")
    delete_thread(session, thread_id)
    return CoderThreadDeleteOut(thread_id=thread_id, deleted=True)


@router.post("/chat/send", response_model=CoderChatSendResponse)
async def coder_chat_send(
    body: CoderChatSendRequest = ...,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "chat:write")
    data = await send_message(
        session,
        body.message,
        thread_id=body.thread_id,
        model=body.model,
        workspace_root=body.workspace_root,
        allow_commands=body.allow_commands,
        auto_approve_commands=body.auto_approve_commands,
        max_tokens=body.max_tokens,
    )
    usage = data.get("usage")
    if usage is not None:
        record_api_token_usage(
            session,
            principal.token_id,
            tokens=usage.total_tokens if hasattr(usage, "total_tokens") else usage.get("total_tokens", 0),
            cost=usage.cost_usd if hasattr(usage, "cost_usd") else usage.get("cost_usd", 0.0),
        )
    return CoderChatSendResponse(**data)


@router.post("/chat/stream")
async def coder_chat_stream(
    body: CoderChatSendRequest = ...,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "chat:write")
    if not llm_proxy_master_key():
        raise HTTPException(status_code=503, detail="AGENT_PLATFORM_MASTER_KEY is not set.")
    generator = stream_message(
        body.message,
        thread_id=body.thread_id,
        model=body.model,
        workspace_root=body.workspace_root,
        allow_commands=body.allow_commands,
        auto_approve_commands=body.auto_approve_commands,
        max_tokens=body.max_tokens,
    )
    tracked = _usage_tracking_stream(generator, session, principal.token_id)
    return StreamingResponse(
        tracked,
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )


@router.post("/chat/approve")
async def coder_chat_approve(
    body: CoderApprovalRequest = ...,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Resolve a run_command call that paused for approval and resume the agent turn."""
    require_scope(principal, "chat:write")
    if not llm_proxy_master_key():
        raise HTTPException(status_code=503, detail="AGENT_PLATFORM_MASTER_KEY is not set.")
    generator = resolve_pending_call(
        thread_id=body.thread_id,
        call_id=body.call_id,
        approve=body.approve,
        edited_command=body.edited_command,
        model=body.model,
        auto_approve_commands=body.auto_approve_commands,
        max_tokens=body.max_tokens,
    )
    tracked = _usage_tracking_stream(generator, session, principal.token_id)
    return StreamingResponse(
        tracked,
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )
