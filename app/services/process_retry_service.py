from __future__ import annotations

from models import Process


def mark_process_for_replanning(process: Process) -> None:
    process.failure_reason = None
    process.status = "planning"


def mark_process_for_execution_retry(process: Process) -> None:
    process.status = "approved"
    process.failure_reason = None
