"""Shared DB updates when a validated planner DAG is applied to a process (approve / retry / auto-approve)."""

import json

from sqlmodel import Session, select

from dag_schema import PlannerDag, planner_dag_to_json_dict
from models import Process, TaskNode


def apply_validated_planner_to_process(session: Session, process_id: int, validated: PlannerDag) -> None:
    """Replace TaskNodes for the process and set ``process.dag_json`` to the canonical JSON string."""
    canonical_json = json.dumps(planner_dag_to_json_dict(validated), ensure_ascii=False)
    old_tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()
    for ot in old_tasks:
        session.delete(ot)
    for agent in validated.subagents:
        task = TaskNode(
            process_id=process_id,
            client_uuid=agent.client_uuid,
            role=agent.role,
            system_prompt=agent.system_prompt,
            instructions=agent.instructions,
            llm_model=agent.llm_model,
            requires_review=agent.requires_review,
        )
        task.dependencies = list(agent.dependencies)
        session.add(task)
    proc = session.get(Process, process_id)
    if proc is None:
        raise ValueError(f"Process {process_id} not found")
    proc.dag_json = canonical_json
