"""Requeue processes interrupted by a server restart.

DAG planning/execution runs on in-process asyncio tasks (BackgroundTasks), so a
restart mid-run strands processes in ``pending``/``planning``/``approved``/``running``
until someone manually calls POST /processes/{id}/sync. This module applies the same
recovery the sync endpoint offers, automatically, once at startup.

Human gates (``approval_required``, ``task_review_required``) and terminal statuses
are left untouched.
"""

from __future__ import annotations

import asyncio
import logging
import os

from sqlmodel import Session, select

import database
from models import Process, TaskNode
from services.event_log_service import append_event
from services.process_sync_service import (
    align_running_process_to_review_required,
    reset_running_tasks_to_pending,
)
from team_schema import team_context_from_snapshot_json

logger = logging.getLogger(__name__)

_RECOVERABLE_STATUSES = ("pending", "planning", "approved", "running")

# Keep strong references so fire-and-forget recovery tasks are not garbage collected.
_recovery_tasks: set[asyncio.Task] = set()


def resume_on_startup_enabled() -> bool:
    raw = (os.getenv("AGENT_PLATFORM_RESUME_ON_STARTUP") or "1").strip().lower()
    return raw in ("1", "true", "yes", "on")


def _spawn(coro) -> None:
    task = asyncio.create_task(coro)
    _recovery_tasks.add(task)
    task.add_done_callback(_recovery_tasks.discard)


async def recover_interrupted_processes() -> dict[str, int]:
    """Requeue interrupted work; returns counts per action for logging/tests."""
    from orchestrator import DAGExecutor

    counts = {"replanned": 0, "requeued": 0, "aligned_review": 0, "skipped": 0}

    # database.engine resolved lazily so tests that monkeypatch it are honored.
    with Session(database.engine) as session:
        procs = session.exec(
            select(Process).where(Process.status.in_(_RECOVERABLE_STATUSES))
        ).all()

        plans: list[tuple[int, str, str | None]] = []
        executions: list[int] = []

        for proc in procs:
            if proc.status in ("pending", "planning"):
                team_context = team_context_from_snapshot_json(proc.team_snapshot_json)
                plans.append((proc.id, proc.goal, team_context))
                append_event(
                    session,
                    process_id=proc.id,
                    event_type="status_change",
                    content="Startup recovery: re-scheduled planning after server restart",
                )
                counts["replanned"] += 1
                continue

            if proc.status == "approved":
                if not (proc.dag_json or "").strip():
                    logger.warning(
                        "Startup recovery: process %s approved without DAG JSON; skipping",
                        proc.id,
                    )
                    counts["skipped"] += 1
                    continue
                executions.append(proc.id)
                append_event(
                    session,
                    process_id=proc.id,
                    event_type="status_change",
                    content="Startup recovery: re-scheduled DAG execution after server restart",
                )
                counts["requeued"] += 1
                continue

            # status == "running"
            tasks = session.exec(
                select(TaskNode).where(TaskNode.process_id == proc.id)
            ).all()
            awaiting_review = sum(1 for t in tasks if t.status == "awaiting_review")
            if awaiting_review:
                align_running_process_to_review_required(proc)
                session.add(proc)
                append_event(
                    session,
                    process_id=proc.id,
                    event_type="status_change",
                    content="Startup recovery: aligned status to task_review_required (review gate open)",
                )
                counts["aligned_review"] += 1
                continue

            reset_n = reset_running_tasks_to_pending(tasks)
            for task in tasks:
                session.add(task)
            proc.failure_reason = None
            session.add(proc)
            executions.append(proc.id)
            append_event(
                session,
                process_id=proc.id,
                event_type="status_change",
                content=(
                    f"Startup recovery: reset {reset_n} stuck running task(s) to pending; "
                    "re-scheduled DAG execution after server restart"
                ),
            )
            counts["requeued"] += 1

        session.commit()

    for process_id, goal, team_context in plans:
        _spawn(DAGExecutor(process_id).plan(goal, team_context))
    for process_id in executions:
        _spawn(DAGExecutor(process_id).execute_dag())

    recovered = counts["replanned"] + counts["requeued"] + counts["aligned_review"]
    if recovered:
        logger.info("Startup recovery: %s", counts)
    return counts


async def run_startup_recovery() -> None:
    """Lifespan entry point; never raises so startup cannot be blocked by recovery."""
    if not resume_on_startup_enabled():
        logger.info("Startup recovery disabled (AGENT_PLATFORM_RESUME_ON_STARTUP)")
        return
    try:
        await recover_interrupted_processes()
    except Exception:
        logger.exception("Startup recovery failed")


def schedule_startup_recovery() -> None:
    """Fire-and-forget from lifespan; the task set keeps a strong reference."""
    _spawn(run_startup_recovery())
