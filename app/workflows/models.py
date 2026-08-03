"""Database models for user-authored workflows (deterministic automations)."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any

from sqlmodel import Field, SQLModel

from time_utils import utc_now_naive


class Workflow(SQLModel, table=True):
    """A user-defined sequence of steps, runnable on demand, via API trigger, or on a schedule."""

    __tablename__ = "workflows"

    id: int | None = Field(default=None, primary_key=True)
    client_id: str | None = Field(default=None, index=True)
    name: str = Field(index=True)
    description: str | None = None
    steps_json: str = Field(default="[]")  # list[WorkflowStep] as JSON
    enabled: bool = Field(default=True)
    # ponytail: interval-only scheduling; add croniter + cron expressions when someone needs "9am weekdays"
    interval_seconds: int | None = Field(default=None)
    next_run_at: datetime | None = Field(default=None, index=True)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_steps(self) -> list[dict[str, Any]]:
        try:
            return json.loads(self.steps_json)
        except json.JSONDecodeError:
            return []

    def set_steps(self, value: list[dict[str, Any]]) -> None:
        self.steps_json = json.dumps(value) if value else "[]"


class WorkflowRun(SQLModel, table=True):
    """One execution of a workflow, with per-step results."""

    __tablename__ = "workflow_runs"

    id: int | None = Field(default=None, primary_key=True)
    workflow_id: int = Field(foreign_key="workflows.id", index=True)
    trigger: str = Field(default="manual")  # manual | api | schedule
    status: str = Field(default="running", index=True)  # running | succeeded | failed
    input_json: str | None = Field(default=None)
    steps_json: str = Field(default="[]")  # list of per-step result dicts
    error: str | None = None
    started_at: datetime = Field(default_factory=utc_now_naive)
    finished_at: datetime | None = None

    def get_input(self) -> dict[str, Any]:
        if not self.input_json:
            return {}
        try:
            return json.loads(self.input_json)
        except json.JSONDecodeError:
            return {}

    def set_input(self, value: dict[str, Any]) -> None:
        self.input_json = json.dumps(value) if value else None

    def get_step_results(self) -> list[dict[str, Any]]:
        try:
            return json.loads(self.steps_json)
        except json.JSONDecodeError:
            return []

    def set_step_results(self, value: list[dict[str, Any]]) -> None:
        self.steps_json = json.dumps(value) if value else "[]"
