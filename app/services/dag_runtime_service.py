from __future__ import annotations

from dataclasses import dataclass

from models import TaskNode
from sqlmodel import Session, select


@dataclass
class DagTaskSnapshot:
    pending_tasks: list[TaskNode]
    awaiting_review_exists: bool
    completed_uuids: set[str]


def load_dag_task_snapshot(session: Session, *, process_id: int) -> DagTaskSnapshot:
    pending_tasks = session.exec(
        select(TaskNode).where(TaskNode.process_id == process_id).where(TaskNode.status == "pending")
    ).all()
    awaiting_review_exists = (
        session.exec(
            select(TaskNode.id)
            .where(TaskNode.process_id == process_id)
            .where(TaskNode.status == "awaiting_review")
        ).first()
        is not None
    )
    completed_tasks = session.exec(
        select(TaskNode).where(TaskNode.process_id == process_id).where(TaskNode.status == "completed")
    ).all()
    completed_uuids = {task.client_uuid for task in completed_tasks}
    return DagTaskSnapshot(
        pending_tasks=pending_tasks,
        awaiting_review_exists=awaiting_review_exists,
        completed_uuids=completed_uuids,
    )


def select_ready_task_ids(
    *,
    pending_tasks: list[TaskNode],
    completed_uuids: set[str],
    max_concurrent: int | None,
) -> list[int]:
    ready_tasks = [
        task for task in pending_tasks if all(dep in completed_uuids for dep in task.dependencies)
    ]
    ready_sorted = sorted(ready_tasks, key=lambda task: task.id)
    batch = ready_sorted if max_concurrent is None else ready_sorted[:max_concurrent]
    return [int(task.id) for task in batch if task.id is not None]
