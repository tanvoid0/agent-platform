"""Pydantic schemas for the workflows API."""

from __future__ import annotations

import re
from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, Field, field_validator, model_validator

_STEP_ID_RE = re.compile(r"^[a-z][a-z0-9_-]{0,63}$")

_HTTP_METHODS = {"GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"}


class WorkflowStep(BaseModel):
    """One step. Steps run sequentially in list order.

    type "http": params = {url, method?, headers?, body?, timeout_seconds?}
    type "action": params = {action_set_id, action_id, arguments?} — the action
    must be server-executable (execution_mode "server" with an endpoint).

    Params may reference earlier data with templates:
    {{trigger.body.<path>}} and {{steps.<step_id>.output.<path>}}.
    """

    id: str
    type: Literal["http", "action"]
    params: dict[str, Any] = Field(default_factory=dict)

    @field_validator("id")
    @classmethod
    def _valid_id(cls, v: str) -> str:
        if not _STEP_ID_RE.fullmatch(v):
            raise ValueError(
                "step id must be a slug: lowercase letter first, then [a-z0-9_-], max 64 chars"
            )
        return v

    @model_validator(mode="after")
    def _valid_params(self) -> "WorkflowStep":
        if self.type == "http":
            url = self.params.get("url")
            if not url or not isinstance(url, str):
                raise ValueError(f"step '{self.id}': http step requires params.url")
            method = str(self.params.get("method", "GET")).upper()
            if method not in _HTTP_METHODS:
                raise ValueError(f"step '{self.id}': unsupported method {method}")
        elif self.type == "action":
            if not self.params.get("action_set_id") or not self.params.get("action_id"):
                raise ValueError(
                    f"step '{self.id}': action step requires params.action_set_id and params.action_id"
                )
        return self


def validate_steps(steps: list[WorkflowStep]) -> None:
    """Cross-step checks: at least one step, unique ids."""
    if not steps:
        raise ValueError("workflow requires at least one step")
    seen: set[str] = set()
    for s in steps:
        if s.id in seen:
            raise ValueError(f"duplicate step id '{s.id}'")
        seen.add(s.id)


class WorkflowCreate(BaseModel):
    name: str
    description: str | None = None
    steps: list[WorkflowStep]
    enabled: bool = True
    interval_seconds: int | None = Field(default=None, ge=60)


class WorkflowUpdate(BaseModel):
    name: str | None = None
    description: str | None = None
    steps: list[WorkflowStep] | None = None
    enabled: bool | None = None
    # ge=60 keeps a typo like interval_seconds=1 from hammering the scheduler
    interval_seconds: int | None = Field(default=None, ge=60)
    clear_interval: bool = False


class WorkflowResponse(BaseModel):
    id: int
    name: str
    description: str | None
    steps: list[WorkflowStep]
    enabled: bool
    interval_seconds: int | None
    next_run_at: datetime | None
    created_at: datetime
    updated_at: datetime


class StepResult(BaseModel):
    id: str
    status: str  # succeeded | failed | skipped
    output: Any = None
    error: str | None = None
    duration_ms: int | None = None


class WorkflowRunResponse(BaseModel):
    id: int
    workflow_id: int
    trigger: str
    status: str
    input: dict[str, Any]
    steps: list[StepResult]
    error: str | None
    started_at: datetime
    finished_at: datetime | None
