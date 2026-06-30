"""Personal Assistant REST routes."""

from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlmodel import Session

from assistant.schemas import (
    ApplyActionsRequest,
    ApplyActionsResponse,
    AssistantResetOut,
    AssistantResetRequest,
    ChatRetryRequest,
    ChatSendRequest,
    ChatSendResponse,
    ChatThreadCreateOut,
    ChatThreadCreateRequest,
    ChatThreadOut,
    ChatThreadsListOut,
    CompleteItemRequest,
    DashboardOut,
    DomainProfileFormsOut,
    DomainProfileOut,
    DomainProfilePatch,
    DomainProfilesOut,
    FormSubmitRequest,
    GoalsOut,
    ReviewApplyRequest,
    ReviewOut,
    ReviewRunRequest,
)
from assistant.services.assistant_chat import (
    create_chat_thread,
    get_thread,
    list_chat_threads,
    retry_chat_message,
    send_chat_message,
    submit_planning_form,
)
from assistant.services.assistant_reset import reset_assistant_workspace
from assistant.services.assistant_service import ensure_assistant_board, get_dashboard, get_goals
from assistant.services.board_action_apply import apply_board_actions, log_item_completion
from assistant.services.review_service import (
    apply_review,
    dismiss_review,
    get_pending_reviews,
    run_review,
)
from assistant.domain_forms import list_domain_form_specs
from assistant.services.user_profile_service import get_all_profiles, get_profile, merge_profile
from database import get_session
from todos.schemas import ItemOut, PlannedActionOut

router = APIRouter(prefix="/assistant", tags=["assistant"])


@router.get("/dashboard", response_model=DashboardOut)
def dashboard(
    project_id: int = Query(..., ge=1),
    horizon: str = Query(default="day"),
    session: Session = Depends(get_session),
):
    data = get_dashboard(session, project_id, horizon=horizon)
    return DashboardOut(**data)


@router.get("/goals", response_model=GoalsOut)
def goals(
    project_id: int = Query(..., ge=1),
    session: Session = Depends(get_session),
):
    return GoalsOut(goals=get_goals(session, project_id))


@router.post("/reset", response_model=AssistantResetOut)
def assistant_reset(
    project_id: int = Query(..., ge=1),
    body: AssistantResetRequest = ...,
    session: Session = Depends(get_session),
):
    if not body.confirm:
        raise HTTPException(
            status_code=400,
            detail="Confirmation required: send confirm=true to reset the assistant workspace",
        )
    data = reset_assistant_workspace(session, project_id)
    return AssistantResetOut(**data)


@router.get("/chat/threads", response_model=ChatThreadsListOut)
def chat_threads_list(
    project_id: int = Query(..., ge=1),
    session: Session = Depends(get_session),
):
    threads = list_chat_threads(session, project_id)
    return ChatThreadsListOut(project_id=project_id, threads=threads)


@router.post("/chat/threads", response_model=ChatThreadCreateOut)
def chat_threads_create(
    project_id: int = Query(..., ge=1),
    body: ChatThreadCreateRequest = ...,
    session: Session = Depends(get_session),
):
    row = create_chat_thread(session, project_id, title=body.title)
    return ChatThreadCreateOut(
        thread_id=row.id,
        project_id=project_id,
        title=row.title or "New chat",
    )


@router.get("/chat/thread", response_model=ChatThreadOut)
async def chat_thread(
    project_id: int = Query(..., ge=1),
    thread_id: int | None = Query(default=None, ge=1),
    session: Session = Depends(get_session),
):
    data = await get_thread(session, project_id, thread_id=thread_id)
    return ChatThreadOut(**data)


@router.post("/chat/send", response_model=ChatSendResponse)
async def chat_send(
    project_id: int = Query(..., ge=1),
    body: ChatSendRequest = ...,
    session: Session = Depends(get_session),
):
    data = await send_chat_message(
        session,
        project_id,
        body.message,
        thread_id=body.thread_id,
        model=body.model,
        delegate_slug=body.delegate_slug,
        propose_actions=body.propose_actions,
    )
    return ChatSendResponse(**data)


@router.post("/chat/retry", response_model=ChatSendResponse)
async def chat_retry(
    project_id: int = Query(..., ge=1),
    body: ChatRetryRequest = ...,
    session: Session = Depends(get_session),
):
    data = await retry_chat_message(
        session,
        project_id,
        body.thread_id,
        body.message_index,
        model=body.model,
        propose_actions=body.propose_actions,
    )
    return ChatSendResponse(**data)


@router.post("/chat/submit-form", response_model=ChatSendResponse)
async def chat_submit_form(
    project_id: int = Query(..., ge=1),
    body: FormSubmitRequest = ...,
    session: Session = Depends(get_session),
):
    data = await submit_planning_form(
        session,
        project_id,
        domain=body.domain,
        answers=body.answers,
        thread_id=body.thread_id,
        auto_continue=body.auto_continue,
        model=body.model,
    )
    return ChatSendResponse(**data)


@router.get("/profile", response_model=DomainProfilesOut)
def list_profiles(
    project_id: int = Query(..., ge=1),
    session: Session = Depends(get_session),
):
    return DomainProfilesOut(project_id=project_id, profiles=get_all_profiles(session, project_id))


@router.get("/profile/forms", response_model=DomainProfileFormsOut)
def list_profile_forms(
    project_id: int = Query(..., ge=1),
):
    return DomainProfileFormsOut(project_id=project_id, forms=list_domain_form_specs())


@router.get("/profile/{domain}", response_model=DomainProfileOut)
def get_domain_profile(
    domain: str,
    project_id: int = Query(..., ge=1),
    session: Session = Depends(get_session),
):
    return DomainProfileOut(
        project_id=project_id,
        domain=domain,
        profile=get_profile(session, project_id, domain),
    )


@router.patch("/profile/{domain}", response_model=DomainProfileOut)
def patch_domain_profile(
    domain: str,
    project_id: int = Query(..., ge=1),
    body: DomainProfilePatch = ...,
    session: Session = Depends(get_session),
):
    profile = merge_profile(session, project_id, domain, body.profile)
    return DomainProfileOut(project_id=project_id, domain=domain, profile=profile)


@router.post("/chat/apply", response_model=ApplyActionsResponse)
def chat_apply(
    project_id: int = Query(..., ge=1),
    body: ApplyActionsRequest = ...,
    session: Session = Depends(get_session),
):
    board = ensure_assistant_board(session, project_id)
    # Apply task actions only — forms are handled via submit-form
    task_actions = [
        a for a in body.actions if a.action_id != "present_planning_form"
    ]
    result = apply_board_actions(session, board.id, task_actions)

    from assistant.services.assistant_chat import _resolve_thread, resolve_thread_proposal
    from time_utils import utc_now_naive

    thread = _resolve_thread(session, project_id, body.thread_id)
    resolve_thread_proposal(thread, "approved" if task_actions else "dismissed")
    thread.set_pending_actions([])
    thread.updated_at = utc_now_naive()
    session.add(thread)
    session.commit()

    return ApplyActionsResponse(
        applied=result.applied,
        skipped=result.skipped,
        created_items=result.created_items,
        updated_items=result.updated_items,
        guidance=result.guidance,
    )


@router.post("/items/{item_id}/complete", response_model=ItemOut)
def complete_item(
    item_id: int,
    body: CompleteItemRequest = ...,
    session: Session = Depends(get_session),
):
    return log_item_completion(
        session,
        item_id,
        time_spent_minutes=body.time_spent_minutes,
        difficulty=body.difficulty,
        notes=body.notes,
        blockers=body.blockers,
    )


@router.post("/reviews/run", response_model=ReviewOut)
async def reviews_run(
    project_id: int = Query(..., ge=1),
    body: ReviewRunRequest = ...,
    session: Session = Depends(get_session),
):
    data = await run_review(session, project_id, model=body.model)
    return ReviewOut(**data)


@router.get("/reviews/pending")
def reviews_pending(
    project_id: int = Query(..., ge=1),
    session: Session = Depends(get_session),
):
    return {"reviews": get_pending_reviews(session, project_id)}


@router.post("/reviews/{review_id}/apply")
def reviews_apply(
    review_id: int,
    body: ReviewApplyRequest = ...,
    session: Session = Depends(get_session),
):
    actions = body.actions
    return apply_review(session, review_id, actions=actions)


@router.post("/reviews/{review_id}/dismiss")
def reviews_dismiss(
    review_id: int,
    session: Session = Depends(get_session),
):
    return dismiss_review(session, review_id)
