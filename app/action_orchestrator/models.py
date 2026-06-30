"""Database models for Action Orchestrator."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any

from sqlmodel import Field, SQLModel

from time_utils import utc_now_naive


class ActionSet(SQLModel, table=True):
    """A collection of actions registered by a client."""

    __tablename__ = "action_sets"

    id: int | None = Field(default=None, primary_key=True)
    client_id: str | None = Field(default=None, index=True)
    name: str = Field(index=True)
    description: str | None = None
    metadata_json: str | None = Field(default=None)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_metadata(self) -> dict[str, Any]:
        if not self.metadata_json:
            return {}
        try:
            return json.loads(self.metadata_json)
        except json.JSONDecodeError:
            return {}

    def set_metadata(self, value: dict[str, Any]) -> None:
        self.metadata_json = json.dumps(value) if value else None


class Action(SQLModel, table=True):
    """An action that a client can perform."""

    __tablename__ = "actions"

    id: int | None = Field(default=None, primary_key=True)
    set_id: int = Field(foreign_key="action_sets.id", index=True)
    action_id: str = Field(index=True)  # Unique within the set
    name: str
    description: str
    parameters_json: str = Field(default="{}")  # JSON Schema
    execution_mode: str = Field(default="client")  # "client" or "server"
    endpoint: str | None = None  # URL for server execution
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_parameters(self) -> dict[str, Any]:
        try:
            return json.loads(self.parameters_json)
        except json.JSONDecodeError:
            return {}

    def set_parameters(self, value: dict[str, Any]) -> None:
        self.parameters_json = json.dumps(value) if value else "{}"


class Session(SQLModel, table=True):
    """A multi-step session with an action set."""

    __tablename__ = "action_sessions"

    id: int | None = Field(default=None, primary_key=True)
    client_id: str | None = Field(default=None, index=True)
    action_set_id: int = Field(foreign_key="action_sets.id", index=True)
    goal: str
    context_json: str | None = Field(default=None)
    status: str = Field(default="active")  # active, paused, completed, failed
    current_step: int = Field(default=0)
    max_steps: int = Field(default=10)
    execution_mode: str = Field(default="client")  # "client" or "server"
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)
    completed_at: datetime | None = None

    def get_context(self) -> dict[str, Any]:
        if not self.context_json:
            return {}
        try:
            return json.loads(self.context_json)
        except json.JSONDecodeError:
            return {}

    def set_context(self, value: dict[str, Any]) -> None:
        self.context_json = json.dumps(value) if value else None


class SessionStep(SQLModel, table=True):
    """A single step/decision within a session."""

    __tablename__ = "session_steps"

    id: int | None = Field(default=None, primary_key=True)
    session_id: int = Field(foreign_key="action_sessions.id", index=True)
    step_number: int = Field(index=True)
    thought: str | None = None
    actions_json: str = Field(default="[]")  # List of PlannedAction
    status: str = Field(default="pending")  # pending, executed, failed, skipped
    created_at: datetime = Field(default_factory=utc_now_naive)
    executed_at: datetime | None = None

    def get_actions(self) -> list[dict[str, Any]]:
        try:
            return json.loads(self.actions_json)
        except json.JSONDecodeError:
            return []

    def set_actions(self, value: list[dict[str, Any]]) -> None:
        self.actions_json = json.dumps(value) if value else "[]"


class SessionResult(SQLModel, table=True):
    """Result of an action execution within a session."""

    __tablename__ = "session_results"

    id: int | None = Field(default=None, primary_key=True)
    session_id: int = Field(foreign_key="action_sessions.id", index=True)
    step_number: int = Field(index=True)
    action_id: str
    result_json: str | None = None
    error: str | None = None
    created_at: datetime = Field(default_factory=utc_now_naive)

    def get_result(self) -> dict[str, Any] | None:
        if not self.result_json:
            return None
        try:
            return json.loads(self.result_json)
        except json.JSONDecodeError:
            return None

    def set_result(self, value: dict[str, Any] | None) -> None:
        self.result_json = json.dumps(value) if value else None
