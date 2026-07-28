"""Registry hook — pipeline stages call register_model_entry; service wires DB persistence."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

_registry_callback: Callable[[dict[str, Any], bool], None] | None = None


def set_registry_callback(cb: Callable[[dict[str, Any], bool], None] | None) -> None:
    global _registry_callback
    _registry_callback = cb


def register_model_entry(entry: dict[str, Any], set_active: bool = True) -> None:
    if _registry_callback is not None:
        _registry_callback(entry, set_active)
