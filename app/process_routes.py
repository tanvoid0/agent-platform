"""Process (multi-agent run) HTTP routes — mounted at `/processes` and `/api/v1/processes`."""

from __future__ import annotations

import asyncio
import json
from datetime import datetime
from typing import Literal

from fastapi import APIRouter, BackgroundTasks, Depends, HTTPException
from pydantic import BaseModel
from sqlmodel import Session, select
from sse_starlette.sse import EventSourceResponse

from api_auth import agent_platform_client_header
from client_scope import (
    assert_process_client_access,
    merged_client_id,
    require_client_id_enabled,
)
from dag_schema import validate_planner_dag
from database import engine, get_session
from models import EventLog, Process, Project, TaskNode, TeamTemplate
from orchestrator import DAGExecutor
from process_approval import apply_validated_planner_to_process
from team_schema import (
    build_process_team_snapshot,
    parse_team_roster_json,
    render_team_context_for_planner,
    team_context_from_snapshot_json,
)

router = APIRouter(tags=["processes"])


class StartProcessRequest(BaseModel):
    goal: str
    auto_approve: bool = False
    team_template_id: int
    project_id: int | None = None
    client_id: str | None = None


class ApproveDagRequest(BaseModel):
    dag_json: str


class ReviewTaskRequest(BaseModel):
    decision: Literal["approve", "reject", "request_changes"]
    output: str | None = None
    feedback: str | None = None
    instructions: str | None = None


@router.get("/processes")
def list_processes(
    session: Session = Depends(get_session),
    limit: int = 50,
    client_id: str | None = None,
    project_id: int | None = None,
    unassigned_only: bool = False,
):
    q = select(Process).order_by(Process.id.desc())
    if client_id is not None and client_id.strip():
        q = q.where(Process.client_id == client_id.strip()[:256])
    if unassigned_only:
        q = q.where(Process.project_id.is_(None))
    elif project_id is not None:
        q = q.where(Process.project_id == project_id)
    rows = session.exec(q.limit(min(limit, 200))).all()
    return {"processes": rows}


@router.post("/processes")
async def start_process(
    req: StartProcessRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    effective = merged_client_id(client_hdr, req.client_id)
    if require_client_id_enabled() and not effective:
        raise HTTPException(
            status_code=400,
            detail="client_id is required (JSON body or X-Agent-Platform-Client header)",
        )

    tmpl = session.get(TeamTemplate, req.team_template_id)
    if not tmpl:
        raise HTTPException(status_code=404, detail="Team template not found")
    if req.project_id is not None:
        proj = session.get(Project, req.project_id)
        if not proj:
            raise HTTPException(status_code=404, detail="Project not found")
    roster = parse_team_roster_json(tmpl.roster_json)
    team_context = render_team_context_for_planner(
        tmpl.name, tmpl.description, tmpl.color, roster
    )
    team_snapshot_json = build_process_team_snapshot(
        tmpl.id,
        tmpl.name,
        tmpl.description,
        tmpl.color,
        roster,
    )

    proc = Process(
        goal=req.goal,
        team_template_id=req.team_template_id,
        team_snapshot_json=team_snapshot_json,
        project_id=req.project_id,
        client_id=effective,
    )
    session.add(proc)
    session.commit()
    session.refresh(proc)

    executor = DAGExecutor(proc.id, auto_approve=req.auto_approve)
    background_tasks.add_task(executor.plan, req.goal, team_context)

    return {"process_id": proc.id, "status": proc.status}


@router.get("/processes/{process_id}")
def get_process_status(
    process_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)

    tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()

    return {"process": proc, "tasks": tasks}


@router.get("/processes/{process_id}/events")
def list_process_events(
    process_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
    event_type: str | None = None,
    limit: int = 500,
    after_id: int = 0,
):
    """Append-ordered event log for a process (trace, status_change, error, …)."""
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    lim = min(max(limit, 1), 2000)
    q = (
        select(EventLog)
        .where(EventLog.process_id == process_id)
        .where(EventLog.id > max(after_id, 0))
    )
    if event_type and event_type.strip():
        q = q.where(EventLog.event_type == event_type.strip())
    logs = session.exec(q.order_by(EventLog.id.asc()).limit(lim)).all()
    return {"events": logs}


@router.post("/processes/{process_id}/approve")
async def approve_dag(
    process_id: int,
    req: ApproveDagRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)

    if proc.status in ("running", "completed", "approved"):
        # "approved" covers duplicate POST after commit but before status moves to "running"
        return {
            "status": proc.status,
            "idempotent": True,
            "message": "DAG already approved or process already finished",
        }

    if proc.status != "approval_required":
        raise HTTPException(
            status_code=400,
            detail=f"Process is not awaiting approval (status={proc.status})",
        )

    try:
        raw = json.loads(req.dag_json)
    except json.JSONDecodeError as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON for approved DAG: {e}") from e

    try:
        validated = validate_planner_dag(raw)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e

    apply_validated_planner_to_process(session, process_id, validated)
    proc.status = "approved"
    session.add(proc)
    session.commit()

    executor = DAGExecutor(process_id)
    background_tasks.add_task(executor.execute_dag)

    return {"status": "approved", "message": "Execution scheduled"}


@router.post("/processes/{process_id}/tasks/{task_id}/review")
async def review_task(
    process_id: int,
    task_id: int,
    req: ReviewTaskRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Approve, reject, or request changes for a task in awaiting_review."""
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    task = session.get(TaskNode, task_id)
    if not task or task.process_id != process_id:
        raise HTTPException(status_code=404, detail="Task not found")

    if task.status == "completed" and req.decision == "approve":
        return {"status": "completed", "idempotent": True}

    if task.status != "awaiting_review":
        raise HTTPException(
            status_code=400,
            detail=f"Task is not awaiting review (status={task.status})",
        )

    if proc.status in ("completed", "failed", "cancelled"):
        raise HTTPException(
            status_code=400,
            detail=f"Process has finished (status={proc.status}); cannot review tasks",
        )

    if req.decision == "request_changes":
        fb = (req.feedback or "").strip()
        if not fb:
            raise HTTPException(status_code=400, detail="feedback is required for request_changes")

        task.draft_output = task.output
        task.output = None
        task.review_feedback = fb
        task.reviewer_client_uuid = None
        task.revision_count += 1
        if req.instructions is not None:
            task.instructions = req.instructions
        task.status = "pending"
        task.failure_debug_json = None
        task.started_at = None
        task.completed_at = None
        task.tokens_used = 0
        proc.status = "running"
        proc.failure_reason = None
        session.add(task)
        session.add(proc)
        session.add(
            EventLog(
                process_id=process_id,
                task_id=task_id,
                event_type="status_change",
                content=f"Task {task.client_uuid} requeued for revision (revision {task.revision_count})",
            )
        )
        session.commit()

        executor = DAGExecutor(process_id)
        background_tasks.add_task(executor.execute_dag)
        return {"status": "requeued", "revision_count": task.revision_count}

    if req.decision == "reject":
        task.reviewer_client_uuid = None
        task.status = "failed"
        task.failure_debug_json = json.dumps(
            {
                "source": "review_reject",
                "message": "Human reviewer rejected this task at the review gate.",
            },
            ensure_ascii=False,
        )
        proc.status = "failed"
        proc.failure_reason = f"Task {task.client_uuid} rejected at review"
        session.add(task)
        session.add(proc)
        session.add(
            EventLog(
                process_id=process_id,
                task_id=task_id,
                event_type="status_change",
                content=f"Task {task.client_uuid} rejected at review",
            )
        )
        session.commit()
        return {"status": "rejected"}

    # approve
    if req.output is not None:
        task.output = req.output
    task.reviewer_client_uuid = None
    task.status = "completed"
    task.completed_at = datetime.utcnow()
    task.draft_output = None
    task.review_feedback = None
    proc.status = "running"
    proc.failure_reason = None
    session.add(task)
    session.add(proc)
    session.add(
        EventLog(
            process_id=process_id,
            task_id=task_id,
            event_type="status_change",
            content=f"Task {task.client_uuid} approved",
        )
    )
    session.commit()

    executor = DAGExecutor(process_id)
    background_tasks.add_task(executor.expand_after_review_approval_and_continue, task_id)
    return {"status": "approved", "message": "Execution scheduled"}


@router.post("/processes/{process_id}/cancel")
def cancel_process(
    process_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    if proc.status in ("completed", "failed", "cancelled"):
        return {"status": proc.status, "idempotent": True}
    if proc.status not in (
        "pending",
        "planning",
        "approval_required",
        "approved",
        "running",
        "task_review_required",
    ):
        raise HTTPException(status_code=400, detail=f"Cannot cancel from status {proc.status}")

    proc.status = "cancelled"
    session.add(proc)
    session.commit()
    return {"status": "cancelled"}


@router.post("/processes/{process_id}/sync")
async def sync_process(
    process_id: int,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """
    Recover a stuck process: re-schedule planning or DAG execution, or explain what blocks progress.

    Use when the UI shows a non-terminal phase but nothing appears to happen (e.g. worker crash,
    server restart). For `running`, this resets any tasks still marked `running` to `pending` and
    re-queues execution—aborting any in-flight work the server no longer tracks.
    """
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)

    tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()

    def task_counts() -> dict[str, int]:
        out: dict[str, int] = {}
        for t in tasks:
            out[t.status] = out.get(t.status, 0) + 1
        return out

    counts = task_counts()
    terminal = ("completed", "failed", "cancelled")

    if proc.status in terminal:
        if proc.status == "failed":
            fin_detail = (
                "Process failed; use POST /retry to re-plan or re-run execution, "
                "or retry individual failed tasks — sync does not apply."
            )
        else:
            fin_detail = "Process is already finished; sync does nothing."
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "none",
            "detail": fin_detail,
            "task_counts": counts,
        }

    if proc.status == "approval_required":
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "blocked",
            "detail": "Waiting for human approval of the planner DAG. Approve in the UI or cancel the process.",
            "task_counts": counts,
        }

    if proc.status == "task_review_required":
        awaiting = counts.get("awaiting_review", 0)
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "blocked",
            "detail": f"Waiting for human task review ({awaiting} task(s) in awaiting_review). Use the review actions on each task.",
            "task_counts": counts,
        }

    # Inconsistent: executor should have set task_review_required when any task awaits review
    if proc.status == "running" and counts.get("awaiting_review", 0) > 0:
        proc.status = "task_review_required"
        proc.failure_reason = None
        session.add(proc)
        session.add(
            EventLog(
                process_id=process_id,
                event_type="status_change",
                content="Sync: aligned process status to task_review_required (review gate open)",
            )
        )
        session.commit()
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "aligned_status",
            "detail": "Process status was running while tasks awaited review; updated to task_review_required.",
            "task_counts": task_counts(),
        }

    if proc.status in ("pending", "planning"):
        team_context = team_context_from_snapshot_json(proc.team_snapshot_json)
        executor = DAGExecutor(process_id)
        background_tasks.add_task(executor.plan, proc.goal, team_context)
        session.add(
            EventLog(
                process_id=process_id,
                event_type="status_change",
                content="Sync: re-scheduled planning",
            )
        )
        session.commit()
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "requeued_plan",
            "detail": "Planning was scheduled again. If planning was already active, you may see duplicate work until one completes.",
            "task_counts": counts,
        }

    if proc.status == "approved":
        if not proc.dag_json or not proc.dag_json.strip():
            raise HTTPException(
                status_code=400,
                detail="Process is approved but has no DAG JSON; cannot re-queue execution.",
            )
        executor = DAGExecutor(process_id)
        background_tasks.add_task(executor.execute_dag)
        session.add(
            EventLog(
                process_id=process_id,
                event_type="status_change",
                content="Sync: re-scheduled DAG execution",
            )
        )
        session.commit()
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "requeued_execution",
            "detail": "DAG execution was scheduled again.",
            "task_counts": counts,
        }

    if proc.status == "running":
        reset_n = 0
        for t in tasks:
            if t.status == "running":
                t.status = "pending"
                t.output = None
                t.draft_output = None
                t.review_feedback = None
                t.reviewer_client_uuid = None
                t.failure_debug_json = None
                t.started_at = None
                t.completed_at = None
                t.tokens_used = 0
                session.add(t)
                reset_n += 1
        proc.failure_reason = None
        session.add(proc)
        session.add(
            EventLog(
                process_id=process_id,
                event_type="status_change",
                content=f"Sync: reset {reset_n} stuck running task(s) to pending; re-scheduled DAG execution",
            )
        )
        session.commit()

        executor = DAGExecutor(process_id)
        background_tasks.add_task(executor.execute_dag)
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "requeued_execution",
            "detail": "DAG execution was scheduled again."
            + (f" Reset {reset_n} task(s) that were still marked running." if reset_n else ""),
            "reset_running_tasks": reset_n,
            "task_counts": task_counts(),
        }

    return {
        "process_id": process_id,
        "process_status": proc.status,
        "action": "none",
        "detail": f"Unexpected process status {proc.status!r}; no automatic recovery.",
        "task_counts": counts,
    }


@router.post("/processes/{process_id}/retry")
async def retry_process(
    process_id: int,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Re-run planning (no tasks persisted) or re-execute from stored DAG after a failed process."""
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    if proc.status != "failed":
        raise HTTPException(
            status_code=400,
            detail=f"Process is not in failed state (status={proc.status})",
        )

    existing_tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()

    if not existing_tasks:
        proc.failure_reason = None
        proc.status = "planning"
        session.add(proc)
        session.add(
            EventLog(
                process_id=process_id,
                event_type="status_change",
                content="Retry: re-planning scheduled",
            )
        )
        session.commit()

        executor = DAGExecutor(process_id)
        team_context = team_context_from_snapshot_json(proc.team_snapshot_json)
        background_tasks.add_task(executor.plan, proc.goal, team_context)
        return {"process_id": process_id, "status": "planning", "retry": "planning"}

    if not proc.dag_json:
        raise HTTPException(
            status_code=400,
            detail="Process has tasks but no stored DAG JSON; cannot retry execution",
        )

    try:
        raw = json.loads(proc.dag_json)
    except json.JSONDecodeError as e:
        raise HTTPException(status_code=400, detail=f"Invalid stored DAG JSON: {e}") from e

    try:
        validated = validate_planner_dag(raw)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e

    apply_validated_planner_to_process(session, process_id, validated)
    proc.status = "approved"
    proc.failure_reason = None
    session.add(proc)
    session.add(
        EventLog(
            process_id=process_id,
            event_type="status_change",
            content="Retry: execution re-scheduled",
        )
    )
    session.commit()

    executor = DAGExecutor(process_id)
    background_tasks.add_task(executor.execute_dag)

    return {"process_id": process_id, "status": "approved", "retry": "execution"}


@router.post("/processes/{process_id}/tasks/{task_id}/retry")
async def retry_failed_task(
    process_id: int,
    task_id: int,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Re-queue one failed task and resume DAG execution (process must be failed)."""
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    task = session.get(TaskNode, task_id)
    if not task or task.process_id != process_id:
        raise HTTPException(status_code=404, detail="Task not found")
    if proc.status != "failed":
        raise HTTPException(
            status_code=400,
            detail=f"Process must be failed to retry a task (status={proc.status})",
        )
    if task.status != "failed":
        raise HTTPException(
            status_code=400,
            detail=f"Task is not failed (status={task.status})",
        )
    if not proc.dag_json:
        raise HTTPException(
            status_code=400,
            detail="Process has no stored DAG JSON; use full process retry instead",
        )

    task.status = "pending"
    task.output = None
    task.draft_output = None
    task.review_feedback = None
    task.reviewer_client_uuid = None
    task.failure_debug_json = None
    task.revision_count = 0
    task.started_at = None
    task.completed_at = None
    task.tokens_used = 0
    proc.failure_reason = None
    proc.status = "approved"
    session.add(task)
    session.add(proc)
    session.add(
        EventLog(
            process_id=process_id,
            task_id=task_id,
            event_type="status_change",
            content=f"Retry: task {task.client_uuid} reset to pending; execution re-scheduled",
        )
    )
    session.commit()

    executor = DAGExecutor(process_id)
    background_tasks.add_task(executor.execute_dag)

    return {"process_id": process_id, "task_id": task_id, "status": "approved", "retry": "task"}


@router.get("/processes/{process_id}/stream")
async def stream_process_events(
    process_id: int,
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """SSE: append-only event log tail (correctness remains on GET /processes/{id})."""

    with Session(engine) as db:
        proc0 = db.get(Process, process_id)
        if not proc0:
            raise HTTPException(status_code=404, detail="Process not found")
        assert_process_client_access(proc0, client_hdr)

    async def event_generator():
        last_log_id = 0
        while True:
            with Session(engine) as db:
                proc = db.get(Process, process_id)
                if not proc:
                    yield {"data": json.dumps({"type": "error", "content": "process not found"})}
                    break

                logs = db.exec(
                    select(EventLog)
                    .where(EventLog.process_id == process_id)
                    .where(EventLog.id > last_log_id)
                    .order_by(EventLog.id.asc())
                ).all()

                for log in logs:
                    last_log_id = log.id
                    payload = {
                        "task_id": log.task_id,
                        "type": log.event_type,
                        "content": log.content,
                        "timestamp": log.created_at.isoformat(),
                    }
                    yield {"data": json.dumps(payload)}

                if proc.status in ("completed", "failed", "cancelled"):
                    if not logs:
                        yield {"data": json.dumps({"type": "terminal", "content": proc.status})}
                    break
                # Stop SSE when human approval is needed — client uses GET /processes/{id} as source of truth.
                if proc.status in ("approval_required", "task_review_required"):
                    yield {"data": json.dumps({"type": "terminal", "content": proc.status})}
                    break

            await asyncio.sleep(0.8)

    return EventSourceResponse(event_generator())
