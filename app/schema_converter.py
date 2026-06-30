"""Utilities for converting ORM models to Pydantic schemas automatically."""

from __future__ import annotations

from typing import Generic, List, Type, TypeVar

from pydantic import BaseModel

T = TypeVar("T", bound=BaseModel)
M = TypeVar("M")


def to_schema(orm_obj: M, schema_class: Type[T]) -> T:
    """Convert ORM object to Pydantic schema using model_validate (respects from_attributes=True)."""
    return schema_class.model_validate(orm_obj)


def to_schemas(orm_objs: List[M], schema_class: Type[T]) -> list[T]:
    """Convert list of ORM objects to Pydantic schemas."""
    return [to_schema(obj, schema_class) for obj in orm_objs]
