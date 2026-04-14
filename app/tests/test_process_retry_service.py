from __future__ import annotations

from models import Process
from services.process_retry_service import (
    mark_process_for_execution_retry,
    mark_process_for_replanning,
)


def test_mark_process_for_replanning_sets_planning_and_clears_failure():
    process = Process(goal="g", status="failed", failure_reason="boom")
    mark_process_for_replanning(process)
    assert process.status == "planning"
    assert process.failure_reason is None


def test_mark_process_for_execution_retry_sets_approved_and_clears_failure():
    process = Process(goal="g", status="failed", failure_reason="boom")
    mark_process_for_execution_retry(process)
    assert process.status == "approved"
    assert process.failure_reason is None
