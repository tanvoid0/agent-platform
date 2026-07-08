"""CRUD services for todo boards."""

from __future__ import annotations

from fastapi import HTTPException
from sqlmodel import Session, select, func

from time_utils import utc_now_naive
from todos.board_templates import get_board_template
from team_schema import random_palette_color
from todos.models import TODO_STATUSES, PlannerAgentProfile, TodoBoard, TodoCategory, TodoItem
from todos.schemas import (
    BoardCreate,
    BoardUpdate,
    CategoryCreate,
    CategoryUpdate,
    CategoryOut,
    ItemCreate,
    ItemOut,
    ItemUpdate,
    BoardOut,
    BoardDetailOut,
    PlannerProfileOut,
)


def _item_out(item: TodoItem) -> ItemOut:
    return ItemOut(
        id=item.id,
        board_id=item.board_id,
        category_id=item.category_id,
        title=item.title,
        description=item.description,
        status=item.status,
        priority=item.priority,
        tags=item.get_tags(),
        plan=item.get_plan(),
        metadata=item.get_metadata(),
        assigned_profile_id=item.assigned_profile_id,
        linked_process_id=item.linked_process_id,
        parent_item_id=item.parent_item_id,
        due_at=item.due_at,
        scheduled_at=item.scheduled_at,
        time_horizon=item.time_horizon,
        item_kind=item.item_kind,
        recurrence=item.get_recurrence(),
        completion=item.get_completion(),
        created_at=item.created_at,
        updated_at=item.updated_at,
    )


def _category_out(cat: TodoCategory) -> CategoryOut:
    return CategoryOut.model_validate(cat)


def _board_out(board: TodoBoard, session: Session) -> BoardOut:
    cat_count = session.exec(
        select(func.count()).select_from(TodoCategory).where(TodoCategory.board_id == board.id)
    ).one()
    item_count = session.exec(
        select(func.count()).select_from(TodoItem).where(TodoItem.board_id == board.id)
    ).one()
    return BoardOut(
        id=board.id,
        project_id=board.project_id,
        name=board.name,
        description=board.description,
        default_model=board.default_model,
        created_at=board.created_at,
        updated_at=board.updated_at,
        category_count=int(cat_count),
        item_count=int(item_count),
    )


def list_boards(session: Session, project_id: int | None = None) -> list[BoardOut]:
    stmt = select(TodoBoard).order_by(TodoBoard.id.asc())
    if project_id is not None:
        stmt = stmt.where(TodoBoard.project_id == project_id)
    rows = session.exec(stmt).all()
    return [_board_out(b, session) for b in rows]


def get_board(session: Session, board_id: int) -> BoardDetailOut:
    from todos.services.planning_context import record_board_visit

    board = session.get(TodoBoard, board_id)
    if not board:
        raise HTTPException(status_code=404, detail="Board not found")
    record_board_visit(session, board_id)
    categories = session.exec(
        select(TodoCategory)
        .where(TodoCategory.board_id == board_id)
        .order_by(TodoCategory.sort_order.asc(), TodoCategory.id.asc())
    ).all()
    items = session.exec(
        select(TodoItem)
        .where(TodoItem.board_id == board_id)
        .order_by(TodoItem.updated_at.desc())
    ).all()
    base = _board_out(board, session)
    return BoardDetailOut(
        **base.model_dump(),
        categories=[_category_out(c) for c in categories],
        items=[_item_out(i) for i in items],
    )


def create_board(session: Session, req: BoardCreate, project_id: int | None = None) -> BoardOut:
    now = utc_now_naive()
    board = TodoBoard(
        project_id=project_id,
        name=req.name.strip(),
        description=req.description.strip() if req.description else None,
        default_model=req.default_model,
        created_at=now,
        updated_at=now,
    )
    session.add(board)
    session.commit()
    session.refresh(board)
    if req.template_slug:
        apply_board_template(session, board.id, req.template_slug.strip())
    return _board_out(board, session)


def apply_board_template(session: Session, board_id: int, template_slug: str) -> None:
    template = get_board_template(template_slug)
    if not template:
        raise HTTPException(status_code=400, detail=f"Unknown board template: {template_slug}")

    _require_board(session, board_id)
    profiles = {
        p.slug: p
        for p in session.exec(select(PlannerAgentProfile)).all()
    }
    now = utc_now_naive()
    for i, cat in enumerate(template.categories):
        profile = profiles.get(cat.profile_slug)
        session.add(
            TodoCategory(
                board_id=board_id,
                name=cat.name,
                color=cat.color,
                sort_order=i,
                planner_profile_id=profile.id if profile else None,
                created_at=now,
                updated_at=now,
            )
        )
    session.commit()


def update_board(session: Session, board_id: int, req: BoardUpdate) -> BoardOut:
    board = session.get(TodoBoard, board_id)
    if not board:
        raise HTTPException(status_code=404, detail="Board not found")
    if req.name is not None:
        board.name = req.name.strip()
    if req.description is not None:
        board.description = req.description.strip() if req.description else None
    if req.default_model is not None:
        board.default_model = req.default_model.strip() if req.default_model else None
    board.updated_at = utc_now_naive()
    session.add(board)
    session.commit()
    session.refresh(board)
    return _board_out(board, session)


def delete_board(session: Session, board_id: int) -> None:
    board = session.get(TodoBoard, board_id)
    if not board:
        raise HTTPException(status_code=404, detail="Board not found")
    session.delete(board)
    session.commit()


def list_categories(session: Session, board_id: int) -> list[CategoryOut]:
    _require_board(session, board_id)
    rows = session.exec(
        select(TodoCategory)
        .where(TodoCategory.board_id == board_id)
        .order_by(TodoCategory.sort_order.asc(), TodoCategory.id.asc())
    ).all()
    return [_category_out(c) for c in rows]


def create_category(session: Session, board_id: int, req: CategoryCreate) -> CategoryOut:
    _require_board(session, board_id)
    now = utc_now_naive()
    cat = TodoCategory(
        board_id=board_id,
        name=req.name.strip(),
        color=req.color or random_palette_color(),
        sort_order=req.sort_order,
        planner_profile_id=req.planner_profile_id,
        created_at=now,
        updated_at=now,
    )
    session.add(cat)
    session.commit()
    session.refresh(cat)
    return _category_out(cat)


def update_category(
    session: Session, board_id: int, category_id: int, req: CategoryUpdate
) -> CategoryOut:
    cat = _require_category(session, board_id, category_id)
    if req.name is not None:
        cat.name = req.name.strip()
    if req.color is not None:
        cat.color = req.color
    if req.sort_order is not None:
        cat.sort_order = req.sort_order
    if req.planner_profile_id is not None:
        cat.planner_profile_id = req.planner_profile_id
    cat.updated_at = utc_now_naive()
    session.add(cat)
    session.commit()
    session.refresh(cat)
    return _category_out(cat)


def list_items(session: Session, board_id: int) -> list[ItemOut]:
    _require_board(session, board_id)
    rows = session.exec(
        select(TodoItem)
        .where(TodoItem.board_id == board_id)
        .order_by(TodoItem.updated_at.desc())
    ).all()
    return [_item_out(i) for i in rows]


def create_item(session: Session, board_id: int, req: ItemCreate) -> ItemOut:
    _require_board(session, board_id)
    if req.status not in TODO_STATUSES:
        raise HTTPException(status_code=400, detail=f"Invalid status: {req.status}")
    if req.category_id is not None:
        _require_category(session, board_id, req.category_id)
    now = utc_now_naive()
    item = TodoItem(
        board_id=board_id,
        category_id=req.category_id,
        title=req.title.strip(),
        description=req.description,
        status=req.status,
        priority=req.priority,
        assigned_profile_id=req.assigned_profile_id,
        parent_item_id=req.parent_item_id,
        due_at=req.due_at,
        scheduled_at=req.scheduled_at,
        time_horizon=req.time_horizon,
        item_kind=req.item_kind or "task",
        created_at=now,
        updated_at=now,
    )
    item.set_tags(req.tags)
    if req.recurrence:
        item.set_recurrence(req.recurrence)
    session.add(item)
    session.commit()
    session.refresh(item)
    return _item_out(item)


def update_item(session: Session, item_id: int, req: ItemUpdate) -> ItemOut:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")
    if req.title is not None:
        item.title = req.title.strip()
    if req.description is not None:
        item.description = req.description
    if req.status is not None:
        if req.status not in TODO_STATUSES:
            raise HTTPException(status_code=400, detail=f"Invalid status: {req.status}")
        item.status = req.status
    if req.category_id is not None:
        _require_category(session, item.board_id, req.category_id)
        item.category_id = req.category_id
    if req.priority is not None:
        item.priority = req.priority
    if req.tags is not None:
        item.set_tags(req.tags)
    if req.plan is not None:
        item.set_plan(req.plan)
    if req.assigned_profile_id is not None:
        item.assigned_profile_id = req.assigned_profile_id
    if req.metadata is not None:
        item.set_metadata(req.metadata)
    if req.parent_item_id is not None:
        item.parent_item_id = req.parent_item_id
    if req.due_at is not None:
        item.due_at = req.due_at
    if req.scheduled_at is not None:
        item.scheduled_at = req.scheduled_at
    if req.time_horizon is not None:
        item.time_horizon = req.time_horizon
    if req.item_kind is not None:
        item.item_kind = req.item_kind
    if req.recurrence is not None:
        item.set_recurrence(req.recurrence)
    if req.completion is not None:
        item.set_completion(req.completion)
    item.updated_at = utc_now_naive()
    session.add(item)
    session.commit()
    session.refresh(item)
    return _item_out(item)


def delete_item(session: Session, item_id: int) -> None:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")
    session.delete(item)
    session.commit()


def get_item(session: Session, item_id: int) -> ItemOut:
    item = session.get(TodoItem, item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Item not found")
    return _item_out(item)


def list_planner_profiles(session: Session) -> list[PlannerProfileOut]:
    rows = session.exec(
        select(PlannerAgentProfile).order_by(PlannerAgentProfile.id.asc())
    ).all()
    return [
        PlannerProfileOut(
            id=p.id,
            slug=p.slug,
            name=p.name,
            requirement_type=p.requirement_type,
            system_prompt=p.system_prompt,
            default_model=p.default_model,
            action_set_id=p.action_set_id,
            skill_paths=p.get_skill_paths(),
        )
        for p in rows
    ]


def resolve_profile_for_item(session: Session, item: TodoItem) -> PlannerAgentProfile | None:
    if item.assigned_profile_id:
        return session.get(PlannerAgentProfile, item.assigned_profile_id)
    if item.category_id:
        cat = session.get(TodoCategory, item.category_id)
        if cat and cat.planner_profile_id:
            return session.get(PlannerAgentProfile, cat.planner_profile_id)
    return session.exec(select(PlannerAgentProfile).order_by(PlannerAgentProfile.id.asc())).first()


def _require_board(session: Session, board_id: int) -> TodoBoard:
    board = session.get(TodoBoard, board_id)
    if not board:
        raise HTTPException(status_code=404, detail="Board not found")
    return board


def _require_category(session: Session, board_id: int, category_id: int) -> TodoCategory:
    cat = session.get(TodoCategory, category_id)
    if not cat or cat.board_id != board_id:
        raise HTTPException(status_code=404, detail="Category not found")
    return cat
