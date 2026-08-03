"""API routes for user-authored workflows.

External apps hit these with a workspace API token; POST /workflows/{id}/run is
the inbound trigger (request body becomes {{trigger.body.*}}).
"""

from __future__ import annotations

import logging
from datetime import timedelta
from typing import Any

from fastapi import APIRouter, Body, Depends, HTTPException
from pydantic import ValidationError
from sqlmodel import Session, select

from action_orchestrator.routes import _check_client_access, action_client_scope
from client_scope import merged_client_id, require_client_id_enabled
from database import get_session
from time_utils import utc_now_naive
from workflows.engine import execute_workflow
from workflows.models import Workflow, WorkflowRun
from workflows.schemas import (
    StepResult,
    WorkflowCreate,
    WorkflowResponse,
    WorkflowRunResponse,
    WorkflowStep,
    WorkflowUpdate,
    validate_steps,
)

logger = logging.getLogger(__name__)
router = APIRouter(tags=["workflows"])


def _workflow_to_response(wf: Workflow) -> WorkflowResponse:
    return WorkflowResponse(
        id=wf.id,
        name=wf.name,
        description=wf.description,
        steps=[WorkflowStep(**s) for s in wf.get_steps()],
        enabled=wf.enabled,
        interval_seconds=wf.interval_seconds,
        next_run_at=wf.next_run_at,
        created_at=wf.created_at,
        updated_at=wf.updated_at,
    )


def _run_to_response(run: WorkflowRun) -> WorkflowRunResponse:
    return WorkflowRunResponse(
        id=run.id,
        workflow_id=run.workflow_id,
        trigger=run.trigger,
        status=run.status,
        input=run.get_input(),
        steps=[StepResult(**r) for r in run.get_step_results()],
        error=run.error,
        started_at=run.started_at,
        finished_at=run.finished_at,
    )


def _get_accessible_workflow(
    workflow_id: int, session: Session, client_hdr: str | None
) -> Workflow:
    wf = session.get(Workflow, workflow_id)
    if not wf:
        raise HTTPException(status_code=404, detail="Workflow not found")
    if not _check_client_access(wf, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")
    return wf


@router.post("/workflows", response_model=WorkflowResponse)
def create_workflow(
    req: WorkflowCreate,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
):
    effective_client = merged_client_id(client_hdr, None)
    if require_client_id_enabled() and not effective_client:
        raise HTTPException(status_code=400, detail="client_id is required")
    try:
        validate_steps(req.steps)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    wf = Workflow(
        client_id=effective_client,
        name=req.name,
        description=req.description,
        enabled=req.enabled,
        interval_seconds=req.interval_seconds,
    )
    wf.set_steps([s.model_dump() for s in req.steps])
    if req.interval_seconds:
        wf.next_run_at = utc_now_naive() + timedelta(seconds=req.interval_seconds)
    session.add(wf)
    session.commit()
    session.refresh(wf)
    return _workflow_to_response(wf)


@router.get("/workflows")
def list_workflows(
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
    limit: int = 50,
):
    query = select(Workflow).order_by(Workflow.id.desc()).limit(limit)
    workflows = session.exec(query).all()
    return {
        "workflows": [
            _workflow_to_response(wf) for wf in workflows if _check_client_access(wf, client_hdr)
        ]
    }


@router.get("/workflows/{workflow_id}", response_model=WorkflowResponse)
def get_workflow(
    workflow_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
):
    return _workflow_to_response(_get_accessible_workflow(workflow_id, session, client_hdr))


@router.put("/workflows/{workflow_id}", response_model=WorkflowResponse)
def update_workflow(
    workflow_id: int,
    req: WorkflowUpdate,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
):
    wf = _get_accessible_workflow(workflow_id, session, client_hdr)
    if req.name is not None:
        wf.name = req.name
    if req.description is not None:
        wf.description = req.description
    if req.steps is not None:
        try:
            validate_steps(req.steps)
        except ValueError as e:
            raise HTTPException(status_code=400, detail=str(e))
        wf.set_steps([s.model_dump() for s in req.steps])
    if req.enabled is not None:
        wf.enabled = req.enabled
    if req.clear_interval:
        wf.interval_seconds = None
        wf.next_run_at = None
    elif req.interval_seconds is not None:
        wf.interval_seconds = req.interval_seconds
        wf.next_run_at = utc_now_naive() + timedelta(seconds=req.interval_seconds)
    wf.updated_at = utc_now_naive()
    session.add(wf)
    session.commit()
    session.refresh(wf)
    return _workflow_to_response(wf)


@router.delete("/workflows/{workflow_id}")
def delete_workflow(
    workflow_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
):
    wf = _get_accessible_workflow(workflow_id, session, client_hdr)
    for run in session.exec(select(WorkflowRun).where(WorkflowRun.workflow_id == wf.id)).all():
        session.delete(run)
    session.delete(wf)
    session.commit()
    return {"success": True}


@router.post("/workflows/{workflow_id}/run", response_model=WorkflowRunResponse)
async def run_workflow(
    workflow_id: int,
    body: dict[str, Any] | None = Body(default=None),
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
):
    """Run now and return the finished run. The JSON body (any shape) is exposed
    to steps as {{trigger.body.*}} — this is the webhook-style trigger for
    external apps."""
    wf = _get_accessible_workflow(workflow_id, session, client_hdr)
    if not wf.enabled:
        raise HTTPException(status_code=400, detail="Workflow is disabled")
    run = await execute_workflow(wf.id, input_data=body or {}, trigger="api")
    return _run_to_response(run)


@router.get("/workflows/{workflow_id}/runs")
def list_runs(
    workflow_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
    limit: int = 20,
):
    _get_accessible_workflow(workflow_id, session, client_hdr)
    runs = session.exec(
        select(WorkflowRun)
        .where(WorkflowRun.workflow_id == workflow_id)
        .order_by(WorkflowRun.id.desc())
        .limit(limit)
    ).all()
    return {"runs": [_run_to_response(r) for r in runs]}


@router.get("/workflows/{workflow_id}/runs/{run_id}", response_model=WorkflowRunResponse)
def get_run(
    workflow_id: int,
    run_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(action_client_scope),
):
    _get_accessible_workflow(workflow_id, session, client_hdr)
    run = session.get(WorkflowRun, run_id)
    if not run or run.workflow_id != workflow_id:
        raise HTTPException(status_code=404, detail="Run not found")
    return _run_to_response(run)
