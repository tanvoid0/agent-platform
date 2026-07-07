"""Pydantic schemas for the standalone chat Playground API."""

from __future__ import annotations

from typing import Any

from chat_usage import ContextUsageOut, LlmUsageOut
from pydantic import BaseModel, Field


class PlaygroundThreadSummary(BaseModel):
    id: int
    title: str
    message_count: int = 0
    preview: str = ""
    created_at: str
    updated_at: str


class PlaygroundThreadsListOut(BaseModel):
    threads: list[PlaygroundThreadSummary] = Field(default_factory=list)


class PlaygroundThreadCreateRequest(BaseModel):
    title: str | None = Field(default=None, max_length=128)


class PlaygroundThreadCreateOut(BaseModel):
    thread_id: int
    title: str


class PlaygroundThreadOut(BaseModel):
    thread_id: int
    title: str = "New chat"
    model: str | None = None
    messages: list[dict[str, Any]] = Field(default_factory=list)
    context_window: int | None = None
    context_usage: ContextUsageOut | None = None


class PlaygroundChatSendRequest(BaseModel):
    message: str = Field(min_length=1)
    thread_id: int | None = None
    model: str | None = None
    temperature: float | None = None
    top_p: float | None = None
    max_tokens: int | None = None


class PlaygroundChatSendResponse(BaseModel):
    thread_id: int
    title: str
    context_window: int
    messages: list[dict[str, Any]] = Field(default_factory=list)
    context_usage: ContextUsageOut | None = None
    usage: LlmUsageOut | None = None


class PlaygroundThreadDeleteOut(BaseModel):
    thread_id: int
    deleted: bool
