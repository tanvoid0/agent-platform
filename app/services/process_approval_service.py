from __future__ import annotations

import json

from dag_schema import validate_planner_dag
from process_approval import apply_validated_planner_to_process
from sqlmodel import Session


def is_idempotent_approval_status(status: str) -> bool:
    return status in ("running", "completed", "approved")


def validate_approved_dag_json(dag_json: str):
    try:
        raw = json.loads(dag_json)
    except json.JSONDecodeError as e:
        raise ValueError(f"Invalid JSON for approved DAG: {e}") from e
    try:
        return validate_planner_dag(raw)
    except ValueError as e:
        raise ValueError(str(e)) from e


def apply_process_approval(session: Session, *, process_id: int, dag_json: str) -> None:
    validated = validate_approved_dag_json(dag_json)
    apply_validated_planner_to_process(session, process_id, validated)
