"""Generic CRUD helpers for consistent error handling and object retrieval."""

from __future__ import annotations

from typing import Generic, Type, TypeVar

from fastapi import HTTPException
from sqlmodel import Session

T = TypeVar("T")


def require_one(session: Session, model_class: Type[T], id_value: int, name: str = "") -> T:
    """Get model by ID, raise 404 if not found. Generic for any model."""
    row = session.get(model_class, id_value)
    if not row:
        resource_name = name or model_class.__name__
        raise HTTPException(status_code=404, detail=f"{resource_name} not found")
    return row


def require_belongs_to(
    session: Session,
    model_class: Type[T],
    id_value: int,
    parent_id_field: str,
    parent_id_value: int,
    name: str = "",
) -> T:
    """Get model by ID, verify it belongs to parent, raise 404 if not found or doesn't belong."""
    row = require_one(session, model_class, id_value, name)
    parent_field_value = getattr(row, parent_id_field, None)
    if parent_field_value != parent_id_value:
        resource_name = name or model_class.__name__
        raise HTTPException(status_code=404, detail=f"{resource_name} not found")
    return row


def require_process_with_access(
    session: Session, process_id: int, client_hdr: str | None = None
) -> T:
    """Get process by ID, verify client access, raise 404/403 if not found or inaccessible."""
    from models import Process
    from client_scope import assert_process_client_access

    proc = require_one(session, Process, process_id, "Process")
    assert_process_client_access(proc, client_hdr)
    return proc
