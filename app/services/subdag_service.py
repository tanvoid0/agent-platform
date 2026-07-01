from __future__ import annotations

import json

from api_tokens.usage_tracking import record_api_token_usage
from dag_schema import merge_planner_with_new_subagents, planner_dag_to_json_dict, sanitize_llm_model_alias
from models import Process, TaskNode
from sqlmodel import Session, select


def merge_and_persist_subdag_expansion(
    session: Session,
    *,
    process_id: int,
    planner,
    new_raw: list[dict],
    add_tokens: int,
    add_cost: float,
    parent_uuid: str,
) -> int:
    merged = merge_planner_with_new_subagents(planner, new_raw)
    merged_json = json.dumps(planner_dag_to_json_dict(merged))

    run = session.get(Process, process_id)
    if not run or run.status != "running":
        return 0
    run.dag_json = merged_json
    run.total_tokens += add_tokens
    run.total_cost += add_cost
    record_api_token_usage(session, run.token_id, tokens=add_tokens, cost=add_cost)

    created = 0
    for agent in new_raw:
        cu = agent["client_uuid"]
        exists = session.exec(
            select(TaskNode)
            .where(TaskNode.process_id == process_id)
            .where(TaskNode.client_uuid == cu)
        ).first()
        if exists:
            continue
        tnode = TaskNode(
            process_id=process_id,
            client_uuid=cu,
            parent_client_uuid=parent_uuid,
            role=agent["role"],
            system_prompt=agent["system_prompt"],
            instructions=agent["instructions"],
            llm_model=sanitize_llm_model_alias(agent.get("model")),
            requires_review=bool(agent.get("requires_review", False)),
        )
        tnode.dependencies = agent.get("dependencies", [])
        session.add(tnode)
        created += 1

    session.commit()
    return created
