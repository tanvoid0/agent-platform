"""Pydantic schemas for Personal Assistant API."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from pydantic import BaseModel, ConfigDict, Field

from todos.schemas import CategoryOut, ItemOut, PlannedActionOut


class DashboardOut(BaseModel):
    project_id: int
    board_id: int
    horizon: str
    range_start: str
    range_end: str
    categories: list[CategoryOut] = Field(default_factory=list)
    items: list[ItemOut] = Field(default_factory=list)
    subtasks_by_parent: dict[int, list[ItemOut]] = Field(default_factory=dict)
    overdue: list[ItemOut] = Field(default_factory=list)
    habits_due: list[ItemOut] = Field(default_factory=list)
    goals: list[ItemOut] = Field(default_factory=list)
    stats: dict[str, Any] = Field(default_factory=dict)


class ChatThreadSummary(BaseModel):
    id: int
    project_id: int
    title: str
    message_count: int = 0
    preview: str = ""
    created_at: str
    updated_at: str


class ChatThreadsListOut(BaseModel):
    project_id: int
    threads: list[ChatThreadSummary] = Field(default_factory=list)


class ChatThreadCreateRequest(BaseModel):
    title: str | None = Field(default=None, max_length=128)


class ChatThreadCreateOut(BaseModel):
    thread_id: int
    project_id: int
    title: str


class ChatThreadOut(BaseModel):
    thread_id: int
    project_id: int
    board_id: int
    title: str = "New chat"
    messages: list[dict[str, Any]] = Field(default_factory=list)
    pending_actions: list[dict[str, Any]] = Field(default_factory=list)
    pending_form: dict[str, Any] | None = None
    last_profile_slug: str | None = None
    domain_profiles: dict[str, dict[str, Any]] = Field(default_factory=dict)


class ChatSendRequest(BaseModel):
    message: str = Field(min_length=1)
    thread_id: int | None = None
    model: str | None = None
    delegate_slug: str | None = None
    propose_actions: bool = True


class ChatRetryRequest(BaseModel):
    thread_id: int = Field(ge=1)
    message_index: int = Field(ge=0, description="Index in stored thread messages (user role)")
    model: str | None = None
    propose_actions: bool = True


class ChatSendResponse(BaseModel):
    thread_id: int | None = None
    content: str
    model: str | None
    profile_slug: str | None
    thought: str | None = None
    actions: list[dict[str, Any]] = Field(default_factory=list)
    messages: list[dict[str, Any]] = Field(default_factory=list)
    pending_actions: list[dict[str, Any]] = Field(default_factory=list)
    pending_form: dict[str, Any] | None = None
    board_id: int
    domain_profiles: dict[str, dict[str, Any]] = Field(default_factory=dict)


class FormSubmitRequest(BaseModel):
    domain: str = Field(min_length=1, max_length=64)
    answers: dict[str, Any] = Field(default_factory=dict)
    thread_id: int | None = None
    auto_continue: bool = True
    model: str | None = None


class DomainProfileOut(BaseModel):
    project_id: int
    domain: str
    profile: dict[str, Any] = Field(default_factory=dict)


class DomainProfilesOut(BaseModel):
    project_id: int
    profiles: dict[str, dict[str, Any]] = Field(default_factory=dict)


class DomainProfileFormsOut(BaseModel):
    project_id: int
    forms: dict[str, dict[str, Any]] = Field(default_factory=dict)


class DomainProfilePatch(BaseModel):
    profile: dict[str, Any] = Field(default_factory=dict)


class ApplyActionsRequest(BaseModel):
    actions: list[PlannedActionOut] = Field(default_factory=list)
    thread_id: int | None = None


class ApplyActionsResponse(BaseModel):
    applied: list[str] = Field(default_factory=list)
    skipped: list[str] = Field(default_factory=list)
    created_items: list[ItemOut] = Field(default_factory=list)
    updated_items: list[ItemOut] = Field(default_factory=list)
    guidance: list[str] = Field(default_factory=list)


class CompleteItemRequest(BaseModel):
    time_spent_minutes: int | None = None
    difficulty: str | None = None
    notes: str | None = None
    blockers: str | None = None


class ReviewRunRequest(BaseModel):
    model: str | None = None


class ReviewOut(BaseModel):
    review_id: int
    status: str
    summary: str | None
    stats: dict[str, Any] = Field(default_factory=dict)
    proposed_actions: list[PlannedActionOut] = Field(default_factory=list)


class ReviewApplyRequest(BaseModel):
    model_config = ConfigDict(extra="ignore")
    actions: list[PlannedActionOut] | None = None


class GoalsOut(BaseModel):
    goals: list[ItemOut] = Field(default_factory=list)


class AssistantResetRequest(BaseModel):
    """Client must send confirm=true after the user acknowledges the destructive action."""

    confirm: bool = False


class AssistantResetOut(BaseModel):
    project_id: int
    board_id: int
    thread_id: int
