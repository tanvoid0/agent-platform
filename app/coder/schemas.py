"""Pydantic schemas for the Coder agent API."""

from __future__ import annotations

from typing import Any

from chat_usage import ContextUsageOut, LlmUsageOut
from pydantic import BaseModel, Field


class CoderThreadSummary(BaseModel):
    id: int
    title: str
    workspace_root: str | None = None
    message_count: int = 0
    preview: str = ""
    created_at: str
    updated_at: str


class CoderThreadsListOut(BaseModel):
    threads: list[CoderThreadSummary] = Field(default_factory=list)


class CoderThreadCreateRequest(BaseModel):
    title: str | None = Field(default=None, max_length=128)
    workspace_root: str | None = Field(default=None, max_length=1024)


class CoderThreadCreateOut(BaseModel):
    thread_id: int
    title: str
    workspace_root: str | None = None


class CoderThreadOut(BaseModel):
    thread_id: int
    title: str = "New session"
    workspace_root: str | None = None
    model: str | None = None
    messages: list[dict[str, Any]] = Field(default_factory=list)
    context_window: int | None = None
    context_usage: ContextUsageOut | None = None


class CoderChatSendRequest(BaseModel):
    message: str = Field(min_length=1)
    thread_id: int | None = None
    model: str | None = None
    provider: str | None = Field(
        default=None,
        description="LLM backend id for the embedded proxy (ollama, lm_studio, gemini, …).",
    )
    workspace_root: str | None = Field(default=None, max_length=1024)
    allow_commands: bool = False
    # When False (default), a run_command tool call pauses the turn and
    # awaits a decision via POST /coder/chat/approve instead of executing.
    auto_approve_commands: bool = False
    max_tokens: int | None = None
    # When True, workspace tools run on the client host (Portal Desktop).
    delegate_tools: bool = False


class CoderChatSendResponse(BaseModel):
    thread_id: int
    title: str
    workspace_root: str | None = None
    context_window: int
    messages: list[dict[str, Any]] = Field(default_factory=list)
    pending_call: dict[str, Any] | None = None
    context_usage: ContextUsageOut | None = None
    usage: LlmUsageOut | None = None


class CoderRetryRequest(BaseModel):
    thread_id: int = Field(ge=1)
    model: str | None = None
    provider: str | None = None
    workspace_root: str | None = Field(default=None, max_length=1024)
    allow_commands: bool = False
    auto_approve_commands: bool = False
    max_tokens: int | None = None
    delegate_tools: bool = False


class CoderApprovalRequest(BaseModel):
    thread_id: int
    call_id: str
    approve: bool
    edited_command: str | None = None
    model: str | None = None
    provider: str | None = None
    auto_approve_commands: bool = False
    max_tokens: int | None = None
    delegate_tools: bool = False


class CoderThreadDeleteOut(BaseModel):
    thread_id: int
    deleted: bool


class CoderToolResultRequest(BaseModel):
    thread_id: int = Field(ge=1)
    call_id: str = Field(min_length=1)
    result: str = ""
