"""API routes for Action Orchestrator."""

from __future__ import annotations

import logging
from typing import Any

from fastapi import APIRouter, Depends, HTTPException
from sqlmodel import Session, select

from action_orchestrator.engine import decide_actions
from action_orchestrator.models import Action, ActionSet, Session as ActionSession, SessionResult, SessionStep
from action_orchestrator.registry import (
    action_set_to_dict,
    create_action,
    create_action_set,
    delete_action,
    delete_action_set,
    get_action,
    get_action_by_set_and_id,
    get_action_set,
    get_action_set_with_actions,
    list_action_sets,
    list_actions,
    update_action,
    update_action_set,
)
from action_orchestrator.schemas import (
    ActionCreate,
    ActionResponse,
    ActionResultResponse,
    ActionResultSubmit,
    ActionSetCreate,
    ActionSetResponse,
    ActionSetUpdate,
    ActionUpdate,
    CompleteSessionRequest,
    DecideRequest,
    DecideResponse,
    PlannedAction,
    SessionCreate,
    SessionHistoryResponse,
    SessionResponse,
    StepRequest,
    StepResponse,
)
from api_auth import agent_platform_client_header
from client_scope import merged_client_id, require_client_id_enabled
from database import get_session
from time_utils import utc_now_naive

logger = logging.getLogger(__name__)
router = APIRouter(tags=["action-orchestrator"])


def _check_client_access(obj, client_hdr: str | None) -> bool:
    """Check if client has access to an object."""
    if not obj.client_id:
        return True  # Public objects
    if not client_hdr:
        return False
    return obj.client_id == client_hdr.strip()


# === Action Set Routes ===

@router.post("/action-sets", response_model=ActionSetResponse)
def create_set(
    req: ActionSetCreate,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Create a new action set with optional actions."""
    effective_client = merged_client_id(client_hdr, None)
    if require_client_id_enabled() and not effective_client:
        raise HTTPException(status_code=400, detail="client_id is required")

    action_set = create_action_set(session, req, client_id=effective_client)
    actions = list_actions(session, action_set.id)
    return action_set_to_dict(action_set, actions)


@router.get("/action-sets")
def list_sets(
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
    limit: int = 50,
):
    """List action sets accessible to the client."""
    effective_client = merged_client_id(client_hdr, None)
    sets = list_action_sets(session, client_id=effective_client, limit=limit)
    result = []
    for s in sets:
        if _check_client_access(s, client_hdr):
            actions = list_actions(session, s.id)
            result.append(action_set_to_dict(s, actions))
    return {"action_sets": result}


@router.get("/action-sets/{set_id}", response_model=ActionSetResponse)
def get_set(
    set_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Get an action set by ID."""
    action_set, actions = get_action_set_with_actions(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")
    return action_set_to_dict(action_set, actions)


@router.put("/action-sets/{set_id}", response_model=ActionSetResponse)
def update_set(
    set_id: int,
    req: ActionSetUpdate,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Update an action set."""
    action_set = get_action_set(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    action_set = update_action_set(session, set_id, req)
    actions = list_actions(session, set_id)
    return action_set_to_dict(action_set, actions)


@router.delete("/action-sets/{set_id}")
def delete_set(
    set_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Delete an action set and all its actions."""
    action_set = get_action_set(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    success = delete_action_set(session, set_id)
    return {"success": success}


# === Action Routes ===

@router.post("/action-sets/{set_id}/actions", response_model=ActionResponse)
def add_action(
    set_id: int,
    req: ActionCreate,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Add an action to an action set."""
    action_set = get_action_set(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    try:
        action = create_action(session, set_id, req)
        if not action:
            raise HTTPException(status_code=400, detail="Failed to create action")
        return {
            "id": action.id,
            "action_id": action.action_id,
            "name": action.name,
            "description": action.description,
            "parameters": action.get_parameters(),
            "execution_mode": action.execution_mode,
            "endpoint": action.endpoint,
        }
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.get("/action-sets/{set_id}/actions")
def list_set_actions(
    set_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """List all actions in an action set."""
    action_set = get_action_set(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    actions = list_actions(session, set_id)
    return {
        "actions": [
            {
                "id": a.id,
                "action_id": a.action_id,
                "name": a.name,
                "description": a.description,
                "parameters": a.get_parameters(),
                "execution_mode": a.execution_mode,
                "endpoint": a.endpoint,
            }
            for a in actions
        ]
    }


@router.get("/action-sets/{set_id}/actions/{action_id}", response_model=ActionResponse)
def get_action_detail(
    set_id: int,
    action_id: str,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Get details of a specific action."""
    action_set = get_action_set(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    action = get_action_by_set_and_id(session, set_id, action_id)
    if not action:
        raise HTTPException(status_code=404, detail="Action not found")

    return {
        "id": action.id,
        "action_id": action.action_id,
        "name": action.name,
        "description": action.description,
        "parameters": action.get_parameters(),
        "execution_mode": action.execution_mode,
        "endpoint": action.endpoint,
    }


@router.put("/action-sets/{set_id}/actions/{action_id}", response_model=ActionResponse)
def update_action_detail(
    set_id: int,
    action_id: str,
    req: ActionUpdate,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Update an action."""
    action_set = get_action_set(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    action = get_action_by_set_and_id(session, set_id, action_id)
    if not action:
        raise HTTPException(status_code=404, detail="Action not found")

    action = update_action(session, action.id, req)
    return {
        "id": action.id,
        "action_id": action.action_id,
        "name": action.name,
        "description": action.description,
        "parameters": action.get_parameters(),
        "execution_mode": action.execution_mode,
        "endpoint": action.endpoint,
    }


@router.delete("/action-sets/{set_id}/actions/{action_id}")
def delete_action_endpoint(
    set_id: int,
    action_id: str,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Delete an action from an action set."""
    action_set = get_action_set(session, set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    action = get_action_by_set_and_id(session, set_id, action_id)
    if not action:
        raise HTTPException(status_code=404, detail="Action not found")

    success = delete_action(session, action.id)
    return {"success": success}


# === Session Routes ===

@router.post("/sessions", response_model=SessionResponse)
def create_session(
    req: SessionCreate,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Create a new orchestration session."""
    effective_client = merged_client_id(client_hdr, None)
    if require_client_id_enabled() and not effective_client:
        raise HTTPException(status_code=400, detail="client_id is required")

    # Verify action set exists and is accessible
    action_set = get_action_set(session, req.action_set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    # Verify action set has actions
    actions = list_actions(session, req.action_set_id)
    if not actions:
        raise HTTPException(status_code=400, detail="Action set has no actions")

    action_session = ActionSession(
        client_id=effective_client,
        action_set_id=req.action_set_id,
        goal=req.goal,
        execution_mode=req.execution_mode,
        max_steps=req.max_steps,
        status="active",
        current_step=0,
    )
    action_session.set_context(req.context)

    session.add(action_session)
    session.commit()
    session.refresh(action_session)

    return {
        "id": action_session.id,
        "action_set_id": action_session.action_set_id,
        "goal": action_session.goal,
        "context": action_session.get_context(),
        "status": action_session.status,
        "current_step": action_session.current_step,
        "max_steps": action_session.max_steps,
        "execution_mode": action_session.execution_mode,
    }


@router.get("/sessions/{session_id}", response_model=SessionResponse)
def get_session(
    session_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Get session details."""
    action_session = session.get(ActionSession, session_id)
    if not action_session:
        raise HTTPException(status_code=404, detail="Session not found")
    if not _check_client_access(action_session, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    return {
        "id": action_session.id,
        "action_set_id": action_session.action_set_id,
        "goal": action_session.goal,
        "context": action_session.get_context(),
        "status": action_session.status,
        "current_step": action_session.current_step,
        "max_steps": action_session.max_steps,
        "execution_mode": action_session.execution_mode,
    }


@router.post("/sessions/{session_id}/steps", response_model=StepResponse)
async def request_step(
    session_id: int,
    req: StepRequest,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Request the next action(s) for a session."""
    action_session = session.get(ActionSession, session_id)
    if not action_session:
        raise HTTPException(status_code=404, detail="Session not found")
    if not _check_client_access(action_session, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    if action_session.status not in ("active", "paused"):
        raise HTTPException(
            status_code=400,
            detail=f"Session is {action_session.status}, cannot request new steps",
        )

    # Check step limit
    if action_session.current_step >= action_session.max_steps:
        action_session.status = "completed"
        action_session.completed_at = utc_now_naive()
        session.add(action_session)
        session.commit()
        return StepResponse(
            session_id=session_id,
            step_number=action_session.current_step,
            thought="Maximum steps reached",
            actions=[],
            status="completed",
            execution_mode=action_session.execution_mode,
            is_final=True,
        )

    # Get actions for this session's action set
    actions = list_actions(session, action_session.action_set_id)

    # Build history from previous steps and results
    history = []
    previous_steps = session.exec(
        select(SessionStep).where(SessionStep.session_id == session_id).order_by(SessionStep.step_number)
    ).all()
    for step in previous_steps:
        step_actions = step.get_actions()
        for step_action in step_actions:
            # Find result for this action
            result = session.exec(
                select(SessionResult)
                .where(SessionResult.session_id == session_id)
                .where(SessionResult.step_number == step.step_number)
                .where(SessionResult.action_id == step_action.get("action_id"))
            ).first()
            if result:
                history.append({
                    "action_id": step_action.get("action_id"),
                    "result": result.get_result(),
                    "error": result.error,
                })

    # Merge session context with step context
    merged_context = {**action_session.get_context(), **req.context}

    # Get AI decision
    planned_actions, thought = await decide_actions(
        goal=action_session.goal,
        context=merged_context,
        actions=actions,
        history=history,
    )

    # If no actions returned, session is complete
    if not planned_actions:
        action_session.status = "completed"
        action_session.completed_at = utc_now_naive()
        session.add(action_session)
        session.commit()
        return StepResponse(
            session_id=session_id,
            step_number=action_session.current_step,
            thought=thought or "No further actions needed",
            actions=[],
            status="completed",
            execution_mode=action_session.execution_mode,
            is_final=True,
        )

    # Increment step counter
    action_session.current_step += 1
    action_session.status = "awaiting_execution"
    session.add(action_session)

    # Create step record
    step = SessionStep(
        session_id=session_id,
        step_number=action_session.current_step,
        thought=thought,
        status="pending",
    )
    step.set_actions([a.model_dump() for a in planned_actions])
    session.add(step)
    session.commit()

    return StepResponse(
        session_id=session_id,
        step_number=action_session.current_step,
        thought=thought,
        actions=planned_actions,
        status="awaiting_execution",
        execution_mode=action_session.execution_mode,
        is_final=False,
    )


@router.post("/sessions/{session_id}/results", response_model=ActionResultResponse)
def submit_result(
    session_id: int,
    req: ActionResultSubmit,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Submit the result of an action execution."""
    action_session = session.get(ActionSession, session_id)
    if not action_session:
        raise HTTPException(status_code=404, detail="Session not found")
    if not _check_client_access(action_session, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    # Verify the step exists
    step = session.exec(
        select(SessionStep)
        .where(SessionStep.session_id == session_id)
        .where(SessionStep.step_number == req.step_number)
    ).first()
    if not step:
        raise HTTPException(status_code=404, detail="Step not found")

    # Create result record
    result = SessionResult(
        session_id=session_id,
        step_number=req.step_number,
        action_id=req.action_id,
        error=req.error,
    )
    result.set_result(req.result)
    session.add(result)

    # Update step status
    step.status = "failed" if req.error else "executed"
    step.executed_at = utc_now_naive()
    session.add(step)

    # Update session status back to active
    action_session.status = "active"
    session.add(action_session)
    session.commit()

    # Check if there are more steps or if we're done
    next_available = action_session.current_step < action_session.max_steps

    return ActionResultResponse(
        session_id=session_id,
        step_number=req.step_number,
        action_id=req.action_id,
        status="failed" if req.error else "success",
        next_step_available=next_available,
    )


@router.post("/sessions/{session_id}/complete")
def complete_session(
    session_id: int,
    req: CompleteSessionRequest | None = None,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Mark a session as manually completed."""
    action_session = session.get(ActionSession, session_id)
    if not action_session:
        raise HTTPException(status_code=404, detail="Session not found")
    if not _check_client_access(action_session, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    action_session.status = "completed"
    action_session.completed_at = utc_now_naive()
    session.add(action_session)
    session.commit()

    return {
        "session_id": session_id,
        "status": "completed",
        "summary": req.summary if req else None,
    }


@router.get("/sessions/{session_id}/history", response_model=SessionHistoryResponse)
def get_session_history(
    session_id: int,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """Get full session history with all steps and results."""
    action_session = session.get(ActionSession, session_id)
    if not action_session:
        raise HTTPException(status_code=404, detail="Session not found")
    if not _check_client_access(action_session, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    # Get all steps
    steps = session.exec(
        select(SessionStep).where(SessionStep.session_id == session_id).order_by(SessionStep.step_number)
    ).all()

    # Get all results
    results = session.exec(
        select(SessionResult).where(SessionResult.session_id == session_id)
    ).all()

    session_resp = SessionResponse(
        id=action_session.id,
        action_set_id=action_session.action_set_id,
        goal=action_session.goal,
        context=action_session.get_context(),
        status=action_session.status,
        current_step=action_session.current_step,
        max_steps=action_session.max_steps,
        execution_mode=action_session.execution_mode,
    )

    steps_resp = []
    for step in steps:
        actions_data = step.get_actions()
        planned_actions = [PlannedAction(**a) for a in actions_data]
        steps_resp.append(StepResponse(
            session_id=session_id,
            step_number=step.step_number,
            thought=step.thought,
            actions=planned_actions,
            status=step.status,
            execution_mode=action_session.execution_mode,
            is_final=False,
        ))

    results_resp = [
        ActionResultResponse(
            session_id=session_id,
            step_number=r.step_number,
            action_id=r.action_id,
            status="failed" if r.error else "success",
            next_step_available=False,
        )
        for r in results
    ]

    return SessionHistoryResponse(
        session=session_resp,
        steps=steps_resp,
        results=results_resp,
    )


# === Simple Mode (One-shot Decision) ===

@router.post("/decide", response_model=DecideResponse)
async def decide(
    req: DecideRequest,
    session: Session = Depends(get_session),
    client_hdr: str | None = Depends(agent_platform_client_header),
):
    """One-shot decision without creating a session."""
    # Verify action set exists and is accessible
    action_set = get_action_set(session, req.action_set_id)
    if not action_set:
        raise HTTPException(status_code=404, detail="Action set not found")
    if not _check_client_access(action_set, client_hdr):
        raise HTTPException(status_code=403, detail="Access denied")

    # Get actions
    actions = list_actions(session, req.action_set_id)
    if not actions:
        raise HTTPException(status_code=400, detail="Action set has no actions")

    # Get AI decision
    planned_actions, thought = await decide_actions(
        goal=req.goal,
        context=req.context,
        actions=actions,
        history=None,
    )

    return DecideResponse(
        thought=thought,
        actions=planned_actions,
        execution_mode=req.execution_mode,
    )
