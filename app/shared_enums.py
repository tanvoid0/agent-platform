"""Canonical enum contract shared by backend and generated frontend types."""

from __future__ import annotations

from enum import StrEnum


class ProcessStatus(StrEnum):
    PENDING = "pending"
    PLANNING = "planning"
    APPROVAL_REQUIRED = "approval_required"
    APPROVED = "approved"
    TASK_REVIEW_REQUIRED = "task_review_required"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


class TaskStatus(StrEnum):
    PENDING = "pending"
    RUNNING = "running"
    AWAITING_REVIEW = "awaiting_review"
    COMPLETED = "completed"
    FAILED = "failed"


class ReviewDecision(StrEnum):
    APPROVE = "approve"
    REJECT = "reject"
    REQUEST_CHANGES = "request_changes"


class ProcessRetryMode(StrEnum):
    PLANNING = "planning"
    EXECUTION = "execution"
    TASK = "task"


class ProcessSyncAction(StrEnum):
    NONE = "none"
    BLOCKED = "blocked"
    ALIGNED_STATUS = "aligned_status"
    REQUEUED_PLAN = "requeued_plan"
    REQUEUED_EXECUTION = "requeued_execution"


def enum_values(enum_cls: type[StrEnum]) -> tuple[str, ...]:
    return tuple(member.value for member in enum_cls)

