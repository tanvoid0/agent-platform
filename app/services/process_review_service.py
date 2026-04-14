from __future__ import annotations

from dataclasses import dataclass

from models import Process, TaskNode
from shared_enums import ReviewDecision
from services.process_mutation_service import (
    approve_task_output,
    reject_task_and_fail_process,
    requeue_task_for_changes,
)


@dataclass
class ReviewMutationResult:
    status: str
    event_content: str
    revision_count: int | None = None


def apply_task_review_decision(
    *,
    task: TaskNode,
    process: Process,
    decision: ReviewDecision,
    output: str | None,
    feedback: str | None,
    instructions: str | None,
) -> ReviewMutationResult:
    if decision == ReviewDecision.REQUEST_CHANGES:
        review_feedback = (feedback or "").strip()
        if not review_feedback:
            raise ValueError("feedback is required for request_changes")
        requeue_task_for_changes(
            task=task,
            process=process,
            feedback=review_feedback,
            instructions=instructions,
        )
        return ReviewMutationResult(
            status="requeued",
            event_content=f"Task {task.client_uuid} requeued for revision (revision {task.revision_count})",
            revision_count=task.revision_count,
        )

    if decision == ReviewDecision.REJECT:
        reject_task_and_fail_process(task=task, process=process)
        return ReviewMutationResult(
            status="rejected",
            event_content=f"Task {task.client_uuid} rejected at review",
        )

    approve_task_output(task=task, process=process, output=output)
    return ReviewMutationResult(
        status="approved",
        event_content=f"Task {task.client_uuid} approved",
    )
