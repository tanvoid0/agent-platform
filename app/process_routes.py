"""Process (multi-agent run) HTTP routes — mounted at `/processes` and `/api/v1/processes`."""

from __future__ import annotations

import asyncio
import json

from fastapi import APIRouter, BackgroundTasks, Depends, HTTPException
from pydantic import BaseModel
from sqlmodel import Session, select
from sse_starlette.sse import EventSourceResponse

from api_auth import agent_platform_client_header
from api_tokens.auth import TokenPrincipal, assert_token_project_access, require_scope, require_valid_token
from client_scope import (
    assert_process_client_access,
    merged_client_id,
    require_client_id_enabled,
)
from crud_helpers import require_one, require_process_with_access
from dag_schema import validate_planner_dag
from database import engine, get_session
from models import EventLog, Process, Project, TaskNode, TeamTemplate
from orchestrator import DAGExecutor
from process_approval import apply_validated_planner_to_process
from shared_enums import ReviewDecision
from services.process_approval_service import (
    apply_process_approval,
    is_idempotent_approval_status,
)
from services.process_mutation_service import (
    append_process_event,
    reset_failed_task_for_retry,
)
from services.process_review_service import apply_task_review_decision
from services.process_sync_service import (
    align_running_process_to_review_required,
    reset_running_tasks_to_pending,
    sync_review_gate_detail,
    sync_terminal_detail,
    task_status_counts,
)
from services.process_retry_service import (
    mark_process_for_execution_retry,
    mark_process_for_replanning,
)
from team_schema import (
    assign_missing_accents,
    build_process_team_snapshot,
    parse_team_roster_json,
    render_team_context_for_planner,
    resolved_team_color,
    team_context_from_snapshot_json,
    with_default_accents,
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
    decision: ReviewDecision
    output: str | None = None
    feedback: str | None = None
    instructions: str | None = None


@router.get("/processes")
def list_processes(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
    limit: int = 50,
    client_id: str | None = None,
    project_id: int | None = None,
    unassigned_only: bool = False,
):
    require_scope(principal, "process:read")
    if principal.workspace_id is not None:
        # Workspace-scoped token: must target a project inside its workspace, and
        # cannot list unassigned (workspace-less) processes.
        if project_id is None:
            raise HTTPException(
                status_code=400,
                detail="project_id is required for a workspace-scoped token.",
            )
        assert_token_project_access(principal, project_id, session)
        unassigned_only = False

    # Require explicit scope: must filter by project_id, client_id, or unassigned_only
    if not (client_id or project_id is not None or unassigned_only):
        raise HTTPException(
            status_code=400,
            detail="Must specify one of: project_id, client_id, or unassigned_only=true"
        )

    q = select(Process).order_by(Process.id.desc())
    if client_id is not None and client_id.strip():
        q = q.where(Process.client_id == client_id.strip()[:256])
    if unassigned_only:
        q = q.where(Process.project_id.is_(None))
    elif project_id is not None:
        q = q.where(Process.project_id == project_id)
    rows = session.exec(q.limit(min(limit, 200))).all()
    return {"processes": rows}


@router.post(
    "/processes",
    summary="Start a multi-agent process",
    description=(
        "Kicks off planning (async) for a goal against a team template. Poll GET /processes/{id} "
        "for status. Requires scope process:write; project-scoped tokens are pinned to their own project."
    ),
)
async def start_process(
    req: StartProcessRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "process:write")
    if principal.workspace_id is not None:
        # Workspace-scoped token: the target project must belong to its workspace.
        if req.project_id is None:
            raise HTTPException(
                status_code=400,
                detail="project_id is required for a workspace-scoped token.",
            )
        assert_token_project_access(principal, req.project_id, session)

    effective = merged_client_id(client_hdr, req.client_id)
    if require_client_id_enabled() and not effective:
        raise HTTPException(
            status_code=400,
            detail="client_id is required (JSON body or X-Agent-Platform-Client header)",
        )

    tmpl = require_one(session, TeamTemplate, req.team_template_id, "Team template")
    # Workspace tokens may only use global (NULL) or their own workspace's templates.
    if (
        principal.workspace_id is not None
        and tmpl.workspace_id is not None
        and tmpl.workspace_id != principal.workspace_id
    ):
        raise HTTPException(status_code=404, detail="Team template not found")
    if req.project_id is not None:
        proj = require_one(session, Project, req.project_id, "Project")
    stable_key = str(tmpl.id)
    team_color = resolved_team_color(tmpl.color, stable_key)
    roster = with_default_accents(
        parse_team_roster_json(tmpl.roster_json),
        team_color,
        stable_key=stable_key,
    )
    team_context = render_team_context_for_planner(
        tmpl.name, tmpl.description, team_color, roster
    )
    team_snapshot_json = build_process_team_snapshot(
        tmpl.id,
        tmpl.name,
        tmpl.description,
        team_color,
        roster,
    )

    proc = Process(
        goal=req.goal,
        team_template_id=req.team_template_id,
        team_snapshot_json=team_snapshot_json,
        project_id=req.project_id,
        client_id=effective,
        token_id=principal.token_id,
    )
    session.add(proc)
    session.commit()
    session.refresh(proc)

    executor = DAGExecutor(proc.id, auto_approve=req.auto_approve)
    background_tasks.add_task(executor.plan, req.goal, team_context)

    return {"process_id": proc.id, "status": proc.status}


@router.get("/processes/{process_id}", summary="Get a process's status and tasks")
def get_process_status(
    process_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "process:read")
    proc = require_process_with_access(session, process_id, client_hdr)
    assert_token_project_access(principal, proc.project_id)
    tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()
    return {"process": proc, "tasks": tasks}


@router.get("/processes/{process_id}/events")
def list_process_events(
    process_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
    principal: TokenPrincipal = Depends(require_valid_token),
    event_type: str | None = None,
    limit: int = 500,
    after_id: int = 0,
):
    """Append-ordered event log for a process (trace, status_change, error, …)."""
    require_scope(principal, "process:read")
    proc = require_process_with_access(session, process_id, client_hdr)
    assert_token_project_access(principal, proc.project_id)
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
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "process:write")
    proc = require_process_with_access(session, process_id, client_hdr)
    assert_token_project_access(principal, proc.project_id)

    if is_idempotent_approval_status(proc.status):
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
        apply_process_approval(session, process_id=process_id, dag_json=req.dag_json)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
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
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Approve, reject, or request changes for a task in awaiting_review."""
    require_scope(principal, "process:write")
    proc = require_process_with_access(session, process_id, client_hdr)
    assert_token_project_access(principal, proc.project_id)
    task = require_one(session, TaskNode, task_id, "Task")
    if task.process_id != process_id:
        raise HTTPException(status_code=404, detail="Task not found")

    if task.status == "completed" and req.decision == ReviewDecision.APPROVE:
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

    try:
        mutation = apply_task_review_decision(
            task=task,
            process=proc,
            decision=req.decision,
            output=req.output,
            feedback=req.feedback,
            instructions=req.instructions,
        )
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e

    session.add(task)
    session.add(proc)
    append_process_event(
        session,
        process_id=process_id,
        task_id=task_id,
        event_type="status_change",
        content=mutation.event_content,
    )
    session.commit()

    if mutation.status == "requeued":
        executor = DAGExecutor(process_id)
        background_tasks.add_task(executor.execute_dag)
        return {"status": "requeued", "revision_count": mutation.revision_count}

    if mutation.status == "rejected":
        return {"status": "rejected"}

    executor = DAGExecutor(process_id)
    background_tasks.add_task(executor.expand_after_review_approval_and_continue, task_id)
    return {"status": "approved", "message": "Execution scheduled"}


@router.post("/processes/{process_id}/cancel")
def cancel_process(
    process_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "process:write")
    proc = require_process_with_access(session, process_id, client_hdr)
    assert_token_project_access(principal, proc.project_id)
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
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """
    Recover a stuck process: re-schedule planning or DAG execution, or explain what blocks progress.

    Use when the UI shows a non-terminal phase but nothing appears to happen (e.g. worker crash,
    server restart). For `running`, this resets any tasks still marked `running` to `pending` and
    re-queues execution—aborting any in-flight work the server no longer tracks.
    """
    require_scope(principal, "process:write")
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    assert_token_project_access(principal, proc.project_id)

    tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()

    counts = task_status_counts(tasks)
    terminal = ("completed", "failed", "cancelled")

    if proc.status in terminal:
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "none",
            "detail": sync_terminal_detail(proc.status),
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
            "detail": sync_review_gate_detail(awaiting),
            "task_counts": counts,
        }

    # Inconsistent: executor should have set task_review_required when any task awaits review
    if proc.status == "running" and counts.get("awaiting_review", 0) > 0:
        align_running_process_to_review_required(proc)
        session.add(proc)
        append_process_event(
            session,
            process_id=process_id,
            event_type="status_change",
            content="Sync: aligned process status to task_review_required (review gate open)",
        )
        session.commit()
        return {
            "process_id": process_id,
            "process_status": proc.status,
            "action": "aligned_status",
            "detail": "Process status was running while tasks awaited review; updated to task_review_required.",
            "task_counts": task_status_counts(tasks),
        }

    if proc.status in ("pending", "planning"):
        team_context = team_context_from_snapshot_json(proc.team_snapshot_json)
        executor = DAGExecutor(process_id)
        background_tasks.add_task(executor.plan, proc.goal, team_context)
        append_process_event(
            session,
            process_id=process_id,
            event_type="status_change",
            content="Sync: re-scheduled planning",
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
        append_process_event(
            session,
            process_id=process_id,
            event_type="status_change",
            content="Sync: re-scheduled DAG execution",
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
        reset_n = reset_running_tasks_to_pending(tasks)
        for task in tasks:
            session.add(task)
        proc.failure_reason = None
        session.add(proc)
        append_process_event(
            session,
            process_id=process_id,
            event_type="status_change",
            content=f"Sync: reset {reset_n} stuck running task(s) to pending; re-scheduled DAG execution",
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
            "task_counts": task_status_counts(tasks),
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
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Re-run planning (no tasks persisted) or re-execute from stored DAG after a failed process."""
    require_scope(principal, "process:write")
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    assert_token_project_access(principal, proc.project_id)
    if proc.status != "failed":
        raise HTTPException(
            status_code=400,
            detail=f"Process is not in failed state (status={proc.status})",
        )

    existing_tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()

    if not existing_tasks:
        mark_process_for_replanning(proc)
        session.add(proc)
        append_process_event(
            session,
            process_id=process_id,
            event_type="status_change",
            content="Retry: re-planning scheduled",
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
    mark_process_for_execution_retry(proc)
    session.add(proc)
    append_process_event(
        session,
        process_id=process_id,
        event_type="status_change",
        content="Retry: execution re-scheduled",
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
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Re-queue one failed task and resume DAG execution (process must be failed)."""
    require_scope(principal, "process:write")
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    assert_process_client_access(proc, client_hdr)
    assert_token_project_access(principal, proc.project_id)
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

    reset_failed_task_for_retry(task=task, process=proc)
    session.add(task)
    session.add(proc)
    append_process_event(
        session,
        process_id=process_id,
        task_id=task_id,
        event_type="status_change",
        content=f"Retry: task {task.client_uuid} reset to pending; execution re-scheduled",
    )
    session.commit()

    executor = DAGExecutor(process_id)
    background_tasks.add_task(executor.execute_dag)

    return {"process_id": process_id, "task_id": task_id, "status": "approved", "retry": "task"}


@router.get("/processes/{process_id}/stream")
async def stream_process_events(
    process_id: int,
    client_hdr: str | None = Depends(agent_platform_client_header),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """SSE: append-only event log tail (correctness remains on GET /processes/{id})."""
    require_scope(principal, "process:read")

    with Session(engine) as db:
        proc0 = db.get(Process, process_id)
        if not proc0:
            raise HTTPException(status_code=404, detail="Process not found")
        assert_process_client_access(proc0, client_hdr)
        assert_token_project_access(principal, proc0.project_id)

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
