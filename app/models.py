import json
from datetime import datetime
from typing import Optional, List, Dict, Any
from sqlmodel import SQLModel, Field, Session, create_engine, select

class Run(SQLModel, table=True):
    id: Optional[int] = Field(default=None, primary_key=True)
    goal: str
    status: str = Field(default="pending") # pending, planning, approval_required, running, completed, failed
    dag_json: Optional[str] = Field(default=None) # The JSON representing the execution graph
    total_tokens: int = Field(default=0)
    total_cost: float = Field(default=0.0)
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)

class TaskNode(SQLModel, table=True):
    id: Optional[int] = Field(default=None, primary_key=True)
    run_id: int = Field(foreign_key="run.id")
    client_uuid: str = Field(index=True) # UUID defined in the DAG to resolve dependencies
    role: str
    system_prompt: str
    instructions: str
    dependencies_json: str = Field(default="[]") # JSON list of client_uuids
    status: str = Field(default="pending") # pending, running, completed, failed
    output: Optional[str] = Field(default=None)
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
    run_id: int = Field(foreign_key="run.id", index=True)
    task_id: Optional[int] = Field(default=None, foreign_key="tasknode.id")
    event_type: str # trace, tool_call, status_change, error
    content: str
    created_at: datetime = Field(default_factory=datetime.utcnow)
