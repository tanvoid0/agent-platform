"""Standardized response models for consistent API contracts."""

from __future__ import annotations

from typing import Generic, TypeVar

from pydantic import BaseModel

T = TypeVar("T")


class ListResponse(BaseModel, Generic[T]):
    """Standardized wrapper for list endpoints. Prevents inconsistencies like {items: []} vs bare []."""

    items: list[T]
    total: int | None = None
    limit: int | None = None

    class Config:
        json_schema_extra = {
            "examples": [
                {
                    "items": [],
                    "total": 0,
                }
            ]
        }
