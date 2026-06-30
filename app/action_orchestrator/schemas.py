"""Pydantic schemas for Action Orchestrator API requests and responses."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel, Field


class ActionParameterSchema(BaseModel):
    """JSON Schema for action parameters."""

    type: str = "object"
    properties: dict[str, dict[str, Any]] = Field(default_factory=dict)
    required: list[str] = Field(default_factory=list)


class ActionCreate(BaseModel):
    """Request to create an action."""

    action_id: str = Field(..., description="Unique identifier for this action within the set")
    name: str = Field(..., description="Human-readable name")
    description: str = Field(..., description="Description of what this action does")
    parameters: dict[str, Any] = Field(
        default_factory=dict,
        description="JSON Schema for action parameters",
    )
    execution_mode: str = Field(
        default="client",
        description="Who executes: 'client' (returned) or 'server' (API calls endpoint)",
    )
    endpoint: str | None = Field(
        default=None,
        description="URL to call for server execution mode",
    )


class ActionUpdate(BaseModel):
    """Request to update an action."""

    name: str | None = None
    description: str | None = None
    parameters: dict[str, Any] | None = None
    execution_mode: str | None = None
    endpoint: str | None = None


class ActionResponse(BaseModel):
    """Response containing action details."""

    id: int
    action_id: str
    name: str
    description: str
    parameters: dict[str, Any]
    execution_mode: str
    endpoint: str | None


class ActionSetCreate(BaseModel):
    """Request to create an action set."""

    name: str = Field(..., description="Name for this action set")
    description: str | None = Field(None, description="Optional description")
    actions: list[ActionCreate] = Field(default_factory=list, description="Initial actions")
    metadata: dict[str, Any] = Field(default_factory=dict, description="Custom metadata")


class ActionSetUpdate(BaseModel):
    """Request to update an action set."""

    name: str | None = None
    description: str | None = None
    metadata: dict[str, Any] | None = None


class ActionSetResponse(BaseModel):
    """Response containing action set details."""

    id: int
    name: str
    description: str | None
    actions: list[ActionResponse]
    metadata: dict[str, Any]


class SessionCreate(BaseModel):
    """Request to create a new session."""

    action_set_id: int = Field(..., description="ID of the action set to use")
    goal: str = Field(..., description="The goal or task to accomplish")
    context: dict[str, Any] = Field(
        default_factory=dict,
        description="Additional context for the AI",
    )
    execution_mode: str = Field(
        default="client",
        description="Default execution mode for this session",
    )
    max_steps: int = Field(
        default=10,
        ge=1,
        le=50,
        description="Maximum number of steps allowed",
    )


class SessionResponse(BaseModel):
    """Response containing session details."""

    id: int
    action_set_id: int
    goal: str
    context: dict[str, Any]
    status: str
    current_step: int
    max_steps: int
    execution_mode: str


class StepRequest(BaseModel):
    """Request the next action(s) for a session."""

    context: dict[str, Any] = Field(
        default_factory=dict,
        description="Additional context for this specific step",
    )
    require_confirmation: bool = Field(
        default=False,
        description="If true, AI will ask for confirmation before proceeding",
    )


class PlannedAction(BaseModel):
    """An action planned by the AI for execution."""

    action_id: str = Field(..., description="ID of the action to execute")
    name: str = Field(..., description="Human-readable name of the action")
    parameters: dict[str, Any] = Field(
        default_factory=dict,
        description="Parameters to pass to the action",
    )
    confidence: float = Field(
        default=1.0,
        ge=0.0,
        le=1.0,
        description="AI's confidence in this action (0-1)",
    )
    reasoning: str | None = Field(
        None,
        description="Why this action was chosen",
    )


class StepResponse(BaseModel):
    """Response containing the AI's decision for a step."""

    session_id: int
    step_number: int
    thought: str | None = Field(
        None,
        description="AI's reasoning about what to do",
    )
    actions: list[PlannedAction] = Field(
        default_factory=list,
        description="Actions to execute (in order)",
    )
    status: str = Field(
        ...,
        description="Session status: active, awaiting_execution, completed, failed",
    )
    execution_mode: str = Field(
        ...,
        description="Whether client or server should execute",
    )
    is_final: bool = Field(
        default=False,
        description="If true, this is the final step in the session",
    )


class ActionResultSubmit(BaseModel):
    """Submit the result of an action execution."""

    step_number: int = Field(..., description="Which step this result is for")
    action_id: str = Field(..., description="Which action was executed")
    result: dict[str, Any] = Field(
        default_factory=dict,
        description="Result data from the action execution",
    )
    error: str | None = Field(
        None,
        description="Error message if action failed",
    )


class ActionResultResponse(BaseModel):
    """Response after submitting an action result."""

    session_id: int
    step_number: int
    action_id: str
    status: str
    next_step_available: bool = Field(
        ...,
        description="If true, you can request the next step",
    )


class DecideRequest(BaseModel):
    """One-shot decision request (simple mode, no session)."""

    action_set_id: int = Field(..., description="Action set to use")
    goal: str = Field(..., description="What needs to be accomplished")
    context: dict[str, Any] = Field(
        default_factory=dict,
        description="Context for the decision",
    )
    execution_mode: str = Field(default="client")


class DecideResponse(BaseModel):
    """One-shot decision response."""

    thought: str | None
    actions: list[PlannedAction]
    execution_mode: str


class CompleteSessionRequest(BaseModel):
    """Request to mark a session as complete."""

    summary: str | None = Field(
        None,
        description="Optional summary of what was accomplished",
    )


class SessionHistoryResponse(BaseModel):
    """Full session history with all steps and results."""

    session: SessionResponse
    steps: list[StepResponse]
    results: list[ActionResultResponse]
