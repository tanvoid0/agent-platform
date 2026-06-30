import json
from datetime import datetime
from typing import Optional, List, Dict, Any

from sqlmodel import SQLModel, Field


class TeamTemplate(SQLModel, table=True):
    """Saved team roster used as planner hint and optional process snapshot."""

    id: Optional[int] = Field(default=None, primary_key=True)
    name: str = Field(max_length=256)
    description: Optional[str] = Field(default=None, max_length=4096)
    color: Optional[str] = Field(default=None, max_length=32)
    # Optional library grouping / card chip (e.g. Engineering, Content).
    category: Optional[str] = Field(default=None, max_length=128)
    roster_json: str
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)


class Project(SQLModel, table=True):
    """User-facing grouping for processes (workspace / folder)."""

    id: Optional[int] = Field(default=None, primary_key=True)
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
