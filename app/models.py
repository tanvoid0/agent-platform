import json
from datetime import datetime
from typing import Optional, List, Dict, Any

from sqlmodel import SQLModel, Field


class TeamTemplate(SQLModel, table=True):
    """Saved team roster used as planner hint and optional process snapshot.

    ``workspace_id`` NULL => global template, shared/reusable across all
    workspaces. Set => owned by one workspace; only that workspace (and the
    master key) may see or modify it.
    """

    id: Optional[int] = Field(default=None, primary_key=True)
    workspace_id: Optional[int] = Field(default=None, foreign_key="workspace.id", index=True)
    name: str = Field(max_length=256)
    description: Optional[str] = Field(default=None, max_length=4096)
    color: Optional[str] = Field(default=None, max_length=32)
    # Optional library grouping / card chip (e.g. Engineering, Content).
    category: Optional[str] = Field(default=None, max_length=128)
    roster_json: str
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)


class Workspace(SQLModel, table=True):
    """Top-level tenant: one microservice / one Flow UI deployment / one token.

    Owns projects; isolation boundary for workspace-scoped API tokens.
    """

    __tablename__ = "workspace"

    id: Optional[int] = Field(default=None, primary_key=True)
    name: str = Field(max_length=256)
    slug: str = Field(max_length=128, unique=True, index=True)
    description: Optional[str] = Field(default=None, max_length=4096)
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)


class Project(SQLModel, table=True):
    """User-facing grouping for processes (grouping within a workspace)."""

    id: Optional[int] = Field(default=None, primary_key=True)
    workspace_id: Optional[int] = Field(default=None, foreign_key="workspace.id", index=True)
    name: str = Field(max_length=256)
    description: Optional[str] = Field(default=None, max_length=4096)
    color: Optional[str] = Field(default=None, max_length=32)
    # Browser workspace snapshot (`PersistedProjectPayload` JSON from the Flow UI).
    workspace_payload_json: Optional[str] = Field(default=None)
    # Last opened todo board for this project (server authority for "Continue planning").
    last_todo_board_id: Optional[int] = Field(default=None, foreign_key="todo_boards.id")
    # Default Personal Assistant planning board for this project.
    assistant_board_id: Optional[int] = Field(default=None, foreign_key="todo_boards.id")
    planning_prefs_json: Optional[str] = Field(default=None)
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)


class Process(SQLModel, table=True):
    """One orchestration from user goal through planner, human gates, and DAG execution."""

    id: Optional[int] = Field(default=None, primary_key=True)
    goal: str
    status: str = Field(
        default="pending"
    )  # pending, planning, approval_required, approved, task_review_required, running, completed, failed, cancelled
    dag_json: Optional[str] = Field(default=None)  # The JSON representing the execution graph
    failure_reason: Optional[str] = Field(default=None)  # Set when status becomes failed
    total_tokens: int = Field(default=0)
    total_cost: float = Field(default=0.0)
    tool_invocations_used: int = Field(default=0)
    team_template_id: Optional[int] = Field(default=None, foreign_key="teamtemplate.id")
    team_snapshot_json: Optional[str] = Field(default=None)
    project_id: Optional[int] = Field(default=None, foreign_key="project.id")
    # Optional namespace for external apps (logical isolation; not a security boundary alone).
    client_id: Optional[str] = Field(default=None, max_length=256, index=True)
    # Set when started by a project-scoped API token, for per-token usage attribution.
    token_id: Optional[int] = Field(default=None, foreign_key="api_tokens.id", index=True)
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)


class TaskNode(SQLModel, table=True):
    id: Optional[int] = Field(default=None, primary_key=True)
    process_id: int = Field(foreign_key="process.id")
    client_uuid: str = Field(index=True)  # UUID defined in the DAG to resolve dependencies
    # Set when this row was spawned by sub-DAG expansion (or future planner parent hints).
    parent_client_uuid: Optional[str] = Field(default=None, index=True)
    role: str
    system_prompt: str
    instructions: str
    llm_model: Optional[str] = Field(default=None, max_length=128)
    dependencies_json: str = Field(default="[]")  # JSON list of client_uuids
    status: str = Field(
        default="pending"
    )  # pending, running, awaiting_review, completed, failed
    requires_review: bool = Field(default=False)
    # Peer agent assigned to review this task (idle in DAG); set by executor, cleared on review action.
    reviewer_client_uuid: Optional[str] = Field(default=None, index=True)
    review_feedback: Optional[str] = Field(default=None)
    revision_count: int = Field(default=0)
    draft_output: Optional[str] = Field(default=None)
    output: Optional[str] = Field(default=None)
    # JSON object: exception type/message, optional traceback, source (llm, unexpected, review_reject).
    failure_debug_json: Optional[str] = Field(default=None)
    tokens_used: int = Field(default=0)
    started_at: Optional[datetime] = Field(default=None)
    completed_at: Optional[datetime] = Field(default=None)

    @property
    def dependencies(self) -> List[str]:
        return json.loads(self.dependencies_json)

    @dependencies.setter
    def dependencies(self, value: List[str]):
        self.dependencies_json = json.dumps(value)


class EventLog(SQLModel, table=True):
    id: Optional[int] = Field(default=None, primary_key=True)
    process_id: int = Field(foreign_key="process.id", index=True)
    task_id: Optional[int] = Field(default=None, foreign_key="tasknode.id")
    event_type: str  # trace, tool_call, status_change, error
    content: str
    created_at: datetime = Field(default_factory=datetime.utcnow)


class ApiToken(SQLModel, table=True):
    """Workspace-scoped external API credential (issued from the dashboard, not the master key)."""

    __tablename__ = "api_tokens"

    id: Optional[int] = Field(default=None, primary_key=True)
    workspace_id: int = Field(foreign_key="workspace.id", index=True)
    # Retained nullable for back-compat / optional future single-project tokens; not
    # used by the workspace-only scoping model.
    project_id: Optional[int] = Field(default=None, foreign_key="project.id", index=True)
    name: str = Field(max_length=256)
    # Public-safe display prefix (e.g. "agp_live_a1b2c3d4"); the secret suffix is never stored.
    prefix: str = Field(max_length=32, index=True)
    token_hash: str = Field(max_length=64, unique=True, index=True)  # sha256 hex of the raw token
    scopes_json: str = Field(default="[]")
    status: str = Field(default="active", index=True)  # active, held, revoked
    rate_limit_per_minute: Optional[int] = Field(default=None)  # None = unlimited
    expires_at: Optional[datetime] = Field(default=None)
    last_used_at: Optional[datetime] = Field(default=None)
    revoked_at: Optional[datetime] = Field(default=None)
    revoked_reason: Optional[str] = Field(default=None, max_length=512)
    held_reason: Optional[str] = Field(default=None, max_length=512)
    total_requests: int = Field(default=0)
    total_errors: int = Field(default=0)
    total_tokens: int = Field(default=0)
    total_cost: float = Field(default=0.0)
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)

    @property
    def scopes(self) -> List[str]:
        return json.loads(self.scopes_json)

    @scopes.setter
    def scopes(self, value: List[str]):
        self.scopes_json = json.dumps(value)


class ApiTokenUsageDaily(SQLModel, table=True):
    """Per-token, per-UTC-day usage rollup (one row per token per day)."""

    __tablename__ = "api_token_usage_daily"

    id: Optional[int] = Field(default=None, primary_key=True)
    token_id: int = Field(foreign_key="api_tokens.id", index=True)
    usage_date: str = Field(max_length=10, index=True)  # "YYYY-MM-DD" UTC
    request_count: int = Field(default=0)
    error_count: int = Field(default=0)
    total_tokens: int = Field(default=0)
    total_cost: float = Field(default=0.0)
