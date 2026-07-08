"""Todo board REST API."""

from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query, Response
from sqlmodel import Session

from api_tokens.auth import (
    TokenPrincipal,
    assert_token_board_access,
    assert_token_item_access,
    assert_token_project_access,
    require_scope,
    require_valid_token,
)
from database import get_session
from todos.board_templates import list_board_templates
from todos.schemas import (
    AgentChatRequest,
    AgentChatResponse,
    AgentStepRequest,
    AgentStepResponse,
    ApplyActionsRequest,
    ApplyActionsResponse,
    BoardCreate,
    BoardDetailOut,
    BoardOut,
    BoardUpdate,
    CategoryCreate,
    CategoryOut,
    CategoryUpdate,
    ExportArtifactOut,
    ItemCreate,
    ItemOut,
    ItemUpdate,
    PlannerProfileOut,
    PlanningFormSubmitRequest,
    SpawnProcessRequest,
    SpawnProcessResponse,
    TodoItemEventOut,
)
from todos.services import agent_bridge, board_service
from todos.services.action_apply import apply_planned_actions, submit_planning_form
from todos.services.item_events import list_item_events
from todos.services.process_spawn import spawn_process_for_item

router = APIRouter(prefix="/todos", tags=["todos"])


@router.get("/board-templates")
def list_board_templates_route() -> dict:
    return {"templates": list_board_templates()}


@router.get("/boards")
def list_boards(
    project_id: int | None = Query(default=None, ge=1),
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> dict:
    require_scope(principal, "todos:read")
    if principal.workspace_id is not None:
        if project_id is None:
            raise HTTPException(
                status_code=400, detail="project_id is required for a workspace-scoped token."
            )
        assert_token_project_access(principal, project_id, session)
    return {"boards": board_service.list_boards(session, project_id=project_id)}


@router.post("/boards", status_code=201, response_model=BoardOut)
def create_board(
    req: BoardCreate,
    project_id: int | None = Query(default=None, ge=1),
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> BoardOut:
    require_scope(principal, "todos:write")
    if principal.workspace_id is not None:
        if project_id is None:
            raise HTTPException(
                status_code=400, detail="project_id is required for a workspace-scoped token."
            )
        assert_token_project_access(principal, project_id, session)
    return board_service.create_board(session, req, project_id=project_id)


@router.get("/boards/{board_id}", response_model=BoardDetailOut)
def get_board(
    board_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> BoardDetailOut:
    require_scope(principal, "todos:read")
    assert_token_board_access(principal, board_id, session)
    return board_service.get_board(session, board_id)


@router.patch("/boards/{board_id}", response_model=BoardOut)
def update_board(
    board_id: int,
    req: BoardUpdate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> BoardOut:
    require_scope(principal, "todos:write")
    assert_token_board_access(principal, board_id, session)
    return board_service.update_board(session, board_id, req)


@router.delete("/boards/{board_id}", status_code=204)
def delete_board(
    board_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "todos:write")
    assert_token_board_access(principal, board_id, session)
    board_service.delete_board(session, board_id)
    return Response(status_code=204)


@router.get("/boards/{board_id}/categories")
def list_categories(
    board_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> dict:
    require_scope(principal, "todos:read")
    assert_token_board_access(principal, board_id, session)
    return {"categories": board_service.list_categories(session, board_id)}


@router.post("/boards/{board_id}/categories", status_code=201, response_model=CategoryOut)
def create_category(
    board_id: int,
    req: CategoryCreate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> CategoryOut:
    require_scope(principal, "todos:write")
    assert_token_board_access(principal, board_id, session)
    return board_service.create_category(session, board_id, req)


@router.patch("/boards/{board_id}/categories/{category_id}", response_model=CategoryOut)
def update_category(
    board_id: int,
    category_id: int,
    req: CategoryUpdate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> CategoryOut:
    require_scope(principal, "todos:write")
    assert_token_board_access(principal, board_id, session)
    return board_service.update_category(session, board_id, category_id, req)


@router.get("/boards/{board_id}/items")
def list_items(
    board_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> dict:
    require_scope(principal, "todos:read")
    assert_token_board_access(principal, board_id, session)
    return {"items": board_service.list_items(session, board_id)}


@router.post("/boards/{board_id}/items", status_code=201, response_model=ItemOut)
def create_item(
    board_id: int,
    req: ItemCreate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> ItemOut:
    require_scope(principal, "todos:write")
    assert_token_board_access(principal, board_id, session)
    return board_service.create_item(session, board_id, req)


@router.get("/items/{item_id}", response_model=ItemOut)
def get_item(
    item_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> ItemOut:
    require_scope(principal, "todos:read")
    assert_token_item_access(principal, item_id, session)
    return board_service.get_item(session, item_id)


@router.patch("/items/{item_id}", response_model=ItemOut)
def update_item(
    item_id: int,
    req: ItemUpdate,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> ItemOut:
    require_scope(principal, "todos:write")
    assert_token_item_access(principal, item_id, session)
    return board_service.update_item(session, item_id, req)


@router.delete("/items/{item_id}", status_code=204)
def delete_item(
    item_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "todos:write")
    assert_token_item_access(principal, item_id, session)
    board_service.delete_item(session, item_id)
    return Response(status_code=204)


@router.get("/planner-profiles")
def list_planner_profiles(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> dict:
    require_scope(principal, "todos:read")
    return {"profiles": board_service.list_planner_profiles(session)}


@router.post("/items/{item_id}/agent/step", response_model=AgentStepResponse)
async def item_agent_step(
    item_id: int,
    req: AgentStepRequest,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> AgentStepResponse:
    require_scope(principal, "todos:write")
    assert_token_item_access(principal, item_id, session)
    ctx = dict(req.context)
    if req.document_paths:
        ctx["document_paths"] = list(req.document_paths)
    return await agent_bridge.agent_step(session, item_id, req.goal, req.model, ctx)


@router.post("/items/{item_id}/agent/chat", response_model=AgentChatResponse)
async def item_agent_chat(
    item_id: int,
    req: AgentChatRequest,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> AgentChatResponse:
    require_scope(principal, "todos:write")
    assert_token_item_access(principal, item_id, session)
    return await agent_bridge.agent_chat(
        session, item_id, req.message, req.model, req.history
    )


@router.post("/items/{item_id}/agent/apply", response_model=ApplyActionsResponse)
def item_agent_apply(
    item_id: int,
    req: ApplyActionsRequest,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> ApplyActionsResponse:
    require_scope(principal, "todos:write")
    assert_token_item_access(principal, item_id, session)
    item, result = apply_planned_actions(session, item_id, req.actions)
    return ApplyActionsResponse(
        item=item,
        applied=result.applied,
        skipped=result.skipped,
        guidance=result.guidance,
        exports=[ExportArtifactOut(**e) for e in result.exports],
    )


@router.post("/items/{item_id}/planning-form/submit", response_model=ItemOut)
def item_planning_form_submit(
    item_id: int,
    req: PlanningFormSubmitRequest,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> ItemOut:
    require_scope(principal, "todos:write")
    assert_token_item_access(principal, item_id, session)
    return submit_planning_form(session, item_id, req.form_index, req.answers)


@router.get("/items/{item_id}/events")
def item_events(
    item_id: int,
    after_id: int = 0,
    limit: int = 200,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> dict:
    require_scope(principal, "todos:read")
    assert_token_item_access(principal, item_id, session)
    board_service.get_item(session, item_id)
    rows = list_item_events(session, item_id, after_id=after_id, limit=limit)
    return {
        "events": [
            TodoItemEventOut(
                id=e.id,
                item_id=e.item_id,
                event_type=e.event_type,
                content=e.get_content(),
                created_at=e.created_at,
            )
            for e in rows
        ]
    }


@router.post("/items/{item_id}/spawn-process", status_code=201, response_model=SpawnProcessResponse)
def item_spawn_process(
    item_id: int,
    req: SpawnProcessRequest,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
) -> SpawnProcessResponse:
    require_scope(principal, "todos:write")
    assert_token_item_access(principal, item_id, session)
    return spawn_process_for_item(
        session,
        item_id,
        team_template_id=req.team_template_id,
        goal=req.goal,
        auto_approve=req.auto_approve,
    )
