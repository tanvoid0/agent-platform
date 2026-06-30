"""SQLModel tables for the todo board domain."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any, Optional

from sqlmodel import Field, SQLModel

from time_utils import utc_now_naive

TODO_STATUSES = ("plan", "backlog", "in_progress", "review", "done")
TODO_TIME_HORIZONS = ("day", "week", "month", "goal")
TODO_ITEM_KINDS = ("task", "habit", "goal", "review", "chore")


class PlannerAgentProfile(SQLModel, table=True):
    """Requirement-typed planning agent bound to an action set."""

    __tablename__ = "planner_agent_profiles"

    id: Optional[int] = Field(default=None, primary_key=True)
    slug: str = Field(max_length=64, index=True, unique=True)
    name: str = Field(max_length=256)
    requirement_type: str = Field(max_length=64, index=True)
    system_prompt: str = Field(default="")
    default_model: Optional[str] = Field(default=None, max_length=128)
    action_set_id: Optional[int] = Field(default=None, foreign_key="action_sets.id")
    skill_paths_json: Optional[str] = Field(default=None)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_skill_paths(self) -> list[str]:
        if not self.skill_paths_json:
            return []
        try:
            data = json.loads(self.skill_paths_json)
            return list(data) if isinstance(data, list) else []
        except json.JSONDecodeError:
            return []

    def set_skill_paths(self, paths: list[str]) -> None:
        self.skill_paths_json = json.dumps(paths) if paths else None


class TodoBoard(SQLModel, table=True):
    __tablename__ = "todo_boards"

    id: Optional[int] = Field(default=None, primary_key=True)
    project_id: Optional[int] = Field(default=None, foreign_key="project.id", index=True)
    name: str = Field(max_length=256)
    description: Optional[str] = Field(default=None, max_length=4096)
    default_model: Optional[str] = Field(default="gemma4:31b-cloud", max_length=128)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)


class TodoCategory(SQLModel, table=True):
    __tablename__ = "todo_categories"

    id: Optional[int] = Field(default=None, primary_key=True)
    board_id: int = Field(foreign_key="todo_boards.id", index=True)
    name: str = Field(max_length=128)
    color: Optional[str] = Field(default="#6366f1", max_length=32)
    sort_order: int = Field(default=0)
    planner_profile_id: Optional[int] = Field(
        default=None, foreign_key="planner_agent_profiles.id"
    )
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)


class TodoItem(SQLModel, table=True):
    __tablename__ = "todo_items"

    id: Optional[int] = Field(default=None, primary_key=True)
    board_id: int = Field(foreign_key="todo_boards.id", index=True)
    category_id: Optional[int] = Field(default=None, foreign_key="todo_categories.id")
    title: str = Field(max_length=512)
    description: str = Field(default="")
    status: str = Field(default="plan", max_length=32, index=True)
    priority: int = Field(default=0)
    tags_json: Optional[str] = Field(default=None)
    plan_json: Optional[str] = Field(default=None)
    assigned_profile_id: Optional[int] = Field(
        default=None, foreign_key="planner_agent_profiles.id"
    )
    linked_process_id: Optional[int] = Field(default=None, foreign_key="process.id")
    parent_item_id: Optional[int] = Field(default=None, foreign_key="todo_items.id", index=True)
    due_at: Optional[datetime] = Field(default=None, index=True)
    scheduled_at: Optional[datetime] = Field(default=None, index=True)
    time_horizon: Optional[str] = Field(default=None, max_length=16, index=True)
    item_kind: Optional[str] = Field(default="task", max_length=32)
    recurrence_json: Optional[str] = Field(default=None)
    completion_json: Optional[str] = Field(default=None)
    metadata_json: Optional[str] = Field(default=None)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_metadata(self) -> dict[str, Any]:
        if not self.metadata_json:
            return {}
        try:
            data = json.loads(self.metadata_json)
            return dict(data) if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_metadata(self, data: dict[str, Any]) -> None:
        self.metadata_json = json.dumps(data, ensure_ascii=False) if data else None

    def get_tags(self) -> list[str]:
        if not self.tags_json:
            return []
        try:
            data = json.loads(self.tags_json)
            return list(data) if isinstance(data, list) else []
        except json.JSONDecodeError:
            return []

    def set_tags(self, tags: list[str]) -> None:
        self.tags_json = json.dumps(tags) if tags else None

    def get_plan(self) -> list[dict[str, Any]]:
        if not self.plan_json:
            return []
        try:
            data = json.loads(self.plan_json)
            return list(data) if isinstance(data, list) else []
        except json.JSONDecodeError:
            return []

    def set_plan(self, steps: list[dict[str, Any]]) -> None:
        self.plan_json = json.dumps(steps) if steps else None

    def get_recurrence(self) -> dict[str, Any]:
        if not self.recurrence_json:
            return {}
        try:
            data = json.loads(self.recurrence_json)
            return dict(data) if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_recurrence(self, data: dict[str, Any]) -> None:
        self.recurrence_json = json.dumps(data, ensure_ascii=False) if data else None

    def get_completion(self) -> dict[str, Any]:
        if not self.completion_json:
            return {}
        try:
            data = json.loads(self.completion_json)
            return dict(data) if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_completion(self, data: dict[str, Any]) -> None:
        self.completion_json = json.dumps(data, ensure_ascii=False) if data else None


class TodoItemEvent(SQLModel, table=True):
    """Append-only audit log for todo item agent steps and applied actions."""

    __tablename__ = "todo_item_events"

    id: Optional[int] = Field(default=None, primary_key=True)
    item_id: int = Field(foreign_key="todo_items.id", index=True)
    event_type: str = Field(max_length=64)
    content_json: str = Field(default="{}")
    created_at: datetime = Field(default_factory=utc_now_naive)

    def get_content(self) -> dict[str, Any]:
        try:
            data = json.loads(self.content_json)
            return dict(data) if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_content(self, data: dict[str, Any]) -> None:
        self.content_json = json.dumps(data, ensure_ascii=False)
