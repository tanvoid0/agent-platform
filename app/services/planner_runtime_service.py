from __future__ import annotations

import json

from api_tokens.usage_tracking import record_api_token_usage
from dag_schema import sanitize_llm_model_alias
from models import Process, TaskNode
from sqlmodel import Session


def mark_process_planning(session: Session, *, process_id: int) -> Process:
    run = session.get(Process, process_id)
    if not run:
        raise ValueError("process not found")
    run.status = "planning"
    session.commit()
    return run


def apply_planner_success(
    session: Session,
    *,
    process_id: int,
    dag: dict,
    tokens: int,
    plan_cost: float,
) -> Process:
    run = session.get(Process, process_id)
    if not run:
        raise ValueError("process not found")
    run.dag_json = json.dumps(dag)
    run.total_tokens += tokens
    run.total_cost += plan_cost
    run.failure_reason = None
    run.status = "approval_required"
    record_api_token_usage(session, run.token_id, tokens=tokens, cost=plan_cost)

    for agent in dag.get("subagents", []):
        task = TaskNode(
            process_id=process_id,
            client_uuid=agent["client_uuid"],
            role=agent["role"],
            system_prompt=agent["system_prompt"],
            instructions=agent["instructions"],
            llm_model=sanitize_llm_model_alias(agent.get("model")),
            requires_review=bool(agent.get("requires_review", False)),
        )
        task.dependencies = agent.get("dependencies", [])
        session.add(task)

    session.commit()
    return run


def apply_planner_failure(session: Session, *, process_id: int, reason: str) -> Process:
    run = session.get(Process, process_id)
    if not run:
        raise ValueError("process not found")
    run.status = "failed"
    run.failure_reason = reason
    record_api_token_usage(session, run.token_id, is_error=True)
    session.commit()
    return run
