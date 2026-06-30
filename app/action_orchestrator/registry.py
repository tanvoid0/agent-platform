"""Action registry CRUD operations."""

from __future__ import annotations

from typing import Any

from sqlmodel import Session, select

from action_orchestrator.models import Action, ActionSet
from action_orchestrator.schemas import ActionCreate, ActionSetCreate, ActionSetUpdate, ActionUpdate


def create_action_set(
    session: Session,
    data: ActionSetCreate,
    client_id: str | None = None,
) -> ActionSet:
    """Create a new action set with optional initial actions."""
    action_set = ActionSet(
        client_id=client_id,
        name=data.name,
        description=data.description,
    )
    action_set.set_metadata(data.metadata)
    session.add(action_set)
    session.commit()
    session.refresh(action_set)

    # Create initial actions if provided
    for action_data in data.actions:
        action = Action(
            set_id=action_set.id,
            action_id=action_data.action_id,
            name=action_data.name,
            description=action_data.description,
            execution_mode=action_data.execution_mode,
            endpoint=action_data.endpoint,
        )
        action.set_parameters(action_data.parameters)
        session.add(action)

    session.commit()
    return action_set


def get_action_set(session: Session, set_id: int) -> ActionSet | None:
    """Get an action set by ID."""
    return session.get(ActionSet, set_id)


def get_action_set_with_actions(session: Session, set_id: int) -> tuple[ActionSet | None, list[Action]]:
    """Get an action set and all its actions."""
    action_set = session.get(ActionSet, set_id)
    if not action_set:
        return None, []
    actions = session.exec(select(Action).where(Action.set_id == set_id)).all()
    return action_set, list(actions)


def list_action_sets(
    session: Session,
    client_id: str | None = None,
    limit: int = 50,
) -> list[ActionSet]:
    """List action sets, optionally filtered by client."""
    query = select(ActionSet).order_by(ActionSet.id.desc())
    if client_id:
        query = query.where(ActionSet.client_id == client_id)
    return list(session.exec(query.limit(limit)).all())


def update_action_set(
    session: Session,
    set_id: int,
    data: ActionSetUpdate,
) -> ActionSet | None:
    """Update an action set."""
    action_set = session.get(ActionSet, set_id)
    if not action_set:
        return None

    if data.name is not None:
        action_set.name = data.name
    if data.description is not None:
        action_set.description = data.description
    if data.metadata is not None:
        action_set.set_metadata(data.metadata)

    session.add(action_set)
    session.commit()
    session.refresh(action_set)
    return action_set


def delete_action_set(session: Session, set_id: int) -> bool:
    """Delete an action set and all its actions."""
    action_set = session.get(ActionSet, set_id)
    if not action_set:
        return False

    # Delete associated actions first
    actions = session.exec(select(Action).where(Action.set_id == set_id)).all()
    for action in actions:
        session.delete(action)

    session.delete(action_set)
    session.commit()
    return True


def create_action(
    session: Session,
    set_id: int,
    data: ActionCreate,
) -> Action | None:
    """Create a new action in an action set."""
    action_set = session.get(ActionSet, set_id)
    if not action_set:
        return None

    # Check for duplicate action_id within the set
    existing = session.exec(
        select(Action).where(Action.set_id == set_id).where(Action.action_id == data.action_id)
    ).first()
    if existing:
        raise ValueError(f"Action with action_id '{data.action_id}' already exists in this set")

    action = Action(
        set_id=set_id,
        action_id=data.action_id,
        name=data.name,
        description=data.description,
        execution_mode=data.execution_mode,
        endpoint=data.endpoint,
    )
    action.set_parameters(data.parameters)

    session.add(action)
    session.commit()
    session.refresh(action)
    return action


def get_action(session: Session, action_id: int) -> Action | None:
    """Get an action by ID."""
    return session.get(Action, action_id)


def get_action_by_set_and_id(session: Session, set_id: int, action_id: str) -> Action | None:
    """Get an action by set_id and action_id."""
    return session.exec(
        select(Action).where(Action.set_id == set_id).where(Action.action_id == action_id)
    ).first()


def list_actions(session: Session, set_id: int) -> list[Action]:
    """List all actions in an action set."""
    return list(session.exec(select(Action).where(Action.set_id == set_id)).all())


def update_action(
    session: Session,
    action_id: int,
    data: ActionUpdate,
) -> Action | None:
    """Update an action."""
    action = session.get(Action, action_id)
    if not action:
        return None

    if data.name is not None:
        action.name = data.name
    if data.description is not None:
        action.description = data.description
    if data.parameters is not None:
        action.set_parameters(data.parameters)
    if data.execution_mode is not None:
        action.execution_mode = data.execution_mode
    if data.endpoint is not None:
        action.endpoint = data.endpoint

    session.add(action)
    session.commit()
    session.refresh(action)
    return action


def delete_action(session: Session, action_id: int) -> bool:
    """Delete an action."""
    action = session.get(Action, action_id)
    if not action:
        return False

    session.delete(action)
    session.commit()
    return True


def action_to_dict(action: Action) -> dict[str, Any]:
    """Convert an Action to a dictionary."""
    return {
        "id": action.id,
        "action_id": action.action_id,
        "name": action.name,
        "description": action.description,
        "parameters": action.get_parameters(),
        "execution_mode": action.execution_mode,
        "endpoint": action.endpoint,
    }


def action_set_to_dict(action_set: ActionSet, actions: list[Action]) -> dict[str, Any]:
    """Convert an ActionSet with actions to a dictionary."""
    return {
        "id": action_set.id,
        "name": action_set.name,
        "description": action_set.description,
        "metadata": action_set.get_metadata(),
        "actions": [action_to_dict(a) for a in actions],
    }
