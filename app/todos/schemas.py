"""Pydantic schemas for todo board API."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from pydantic import BaseModel, ConfigDict, Field, field_validator

from todos.models import TODO_ITEM_KINDS, TODO_STATUSES, TODO_TIME_HORIZONS


class PlannerProfileOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    slug: str
    name: str
    requirement_type: str
    system_prompt: str
    default_model: str | None
    action_set_id: int | None
    skill_paths: list[str] = Field(default_factory=list)


class BoardCreate(BaseModel):
    name: str = Field(min_length=1, max_length=256)
    description: str | None = Field(default=None, max_length=4096)
    default_model: str | None = Field(default="gemma4:31b-cloud", max_length=128)
    template_slug: str | None = Field(default=None, max_length=64)


class BoardTemplateOut(BaseModel):
    slug: str
    name: str
    description: str
    categories: list[dict[str, str]] = Field(default_factory=list)


class BoardUpdate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str | None = Field(default=None, min_length=1, max_length=256)
    description: str | None = None
    default_model: str | None = Field(default=None, max_length=128)


class BoardOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    project_id: int | None = None
    name: str
    description: str | None
    default_model: str | None
    created_at: datetime
    updated_at: datetime
    category_count: int = 0
    item_count: int = 0


class CategoryCreate(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    color: str | None = Field(default="#6366f1", max_length=32)
    sort_order: int = 0
    planner_profile_id: int | None = None


class CategoryUpdate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str | None = Field(default=None, min_length=1, max_length=128)
    color: str | None = Field(default=None, max_length=32)
    sort_order: int | None = None
    planner_profile_id: int | None = None


class CategoryOut(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: int
    board_id: int
    name: str
    color: str | None
    sort_order: int
    planner_profile_id: int | None
    created_at: datetime
    updated_at: datetime


class ItemCreate(BaseModel):
    title: str = Field(min_length=1, max_length=512)
    description: str = ""
    status: str = Field(default="plan")
    category_id: int | None = None
    priority: int = 0
    tags: list[str] = Field(default_factory=list)
    assigned_profile_id: int | None = None
    parent_item_id: int | None = None
    due_at: datetime | None = None
    scheduled_at: datetime | None = None
    time_horizon: str | None = None
    item_kind: str | None = Field(default="task")
    recurrence: dict[str, Any] | None = None

    @field_validator("status")
    @classmethod
    def _validate_status(cls, v: str) -> str:
        if v not in TODO_STATUSES:
            raise ValueError(f"status must be one of {TODO_STATUSES}")
        return v

    @field_validator("time_horizon")
    @classmethod
    def _validate_horizon(cls, v: str | None) -> str | None:
        if v is not None and v not in TODO_TIME_HORIZONS:
            raise ValueError(f"time_horizon must be one of {TODO_TIME_HORIZONS}")
        return v

    @field_validator("item_kind")
    @classmethod
    def _validate_kind(cls, v: str | None) -> str | None:
        if v is not None and v not in TODO_ITEM_KINDS:
            raise ValueError(f"item_kind must be one of {TODO_ITEM_KINDS}")
        return v


class ItemUpdate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    title: str | None = Field(default=None, min_length=1, max_length=512)
    description: str | None = None
    status: str | None = None
    category_id: int | None = None
    priority: int | None = None
    tags: list[str] | None = None
    plan: list[dict[str, Any]] | None = None
    assigned_profile_id: int | None = None
    metadata: dict[str, Any] | None = None
    parent_item_id: int | None = None
    due_at: datetime | None = None
    scheduled_at: datetime | None = None
    time_horizon: str | None = None
    item_kind: str | None = None
    recurrence: dict[str, Any] | None = None
    completion: dict[str, Any] | None = None


class ItemOut(BaseModel):
    id: int
    board_id: int
    category_id: int | None
    title: str
    description: str
    status: str
    priority: int
    tags: list[str]
    plan: list[dict[str, Any]]
    metadata: dict[str, Any] = Field(default_factory=dict)
    assigned_profile_id: int | None
    linked_process_id: int | None
    parent_item_id: int | None = None
    due_at: datetime | None = None
    scheduled_at: datetime | None = None
    time_horizon: str | None = None
    item_kind: str | None = None
    recurrence: dict[str, Any] = Field(default_factory=dict)
    completion: dict[str, Any] = Field(default_factory=dict)
    created_at: datetime
    updated_at: datetime


class BoardDetailOut(BoardOut):
    categories: list[CategoryOut] = Field(default_factory=list)
    items: list[ItemOut] = Field(default_factory=list)


class AgentStepRequest(BaseModel):
    goal: str = Field(default="What should I do next for this task?")
    model: str | None = None
    context: dict[str, Any] = Field(default_factory=dict)
    document_paths: list[str] = Field(
        default_factory=list,
        description="Workspace-relative paths (e.g. documents/meal-plan.pdf) included in agent context",
    )


class AgentChatRequest(BaseModel):
    message: str
    model: str | None = None
    history: list[dict[str, str]] = Field(default_factory=list)


class PlannedActionOut(BaseModel):
    action_id: str
    name: str
    parameters: dict[str, Any] = Field(default_factory=dict)
    confidence: float = 1.0
    reasoning: str | None = None


class AgentStepResponse(BaseModel):
    thought: str | None
    actions: list[PlannedActionOut]
    profile_slug: str | None
    action_set_id: int | None


class AgentChatResponse(BaseModel):
    content: str
    model: str | None
    profile_slug: str | None


class ApplyActionsRequest(BaseModel):
    actions: list[PlannedActionOut] = Field(default_factory=list)


class ExportArtifactOut(BaseModel):
    kind: str
    filename: str
    content: str


class ApplyActionsResponse(BaseModel):
    item: ItemOut
    applied: list[str] = Field(default_factory=list)
    skipped: list[str] = Field(default_factory=list)
    guidance: list[str] = Field(default_factory=list)
    exports: list[ExportArtifactOut] = Field(default_factory=list)


class PlanningFormSubmitRequest(BaseModel):
    form_index: int = Field(ge=0)
    answers: dict[str, Any] = Field(default_factory=dict)


class TodoItemEventOut(BaseModel):
    id: int
    item_id: int
    event_type: str
    content: dict[str, Any]
    created_at: datetime


class ProjectPlanningContextOut(BaseModel):
    project_id: int
    last_todo_board_id: int | None
    onboarding_dismissed: bool = False


class ProjectPlanningContextPatch(BaseModel):
    model_config = ConfigDict(extra="ignore")

    last_todo_board_id: int | None = None
    onboarding_dismissed: bool | None = None


class SpawnProcessRequest(BaseModel):
    team_template_id: int = Field(ge=1)
    goal: str | None = None
    auto_approve: bool = False


class SpawnProcessResponse(BaseModel):
    process_id: int
    status: str
    item: ItemOut
    auto_approve: bool
    note: str | None = None
