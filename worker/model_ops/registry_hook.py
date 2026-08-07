"""Report a trained model to the server that spawned this stage.

This used to be a callback the API server installed on import: the pipeline
called ``register_model_entry`` and a SQLAlchemy session two frames up wrote the
row. That is why a training subprocess needed ``database.py``, ``models.py`` and
the ORM, and why the model-ops job routes could not leave Python — the worker
was reaching into the server's database.

Now it prints. ``agent-platformd`` reads the worker's stdout to tee it into the
job log anyway, so a line it can recognise costs nothing, and the registry
tables end up with exactly one writer. The marker is parsed by
``model_ops.rs::handle_marker``; the prefix is duplicated there and must stay in
step with this file.
"""

from __future__ import annotations

import json
import sys
from typing import Any

MARKER = "@@AGP:registry@@"


def register_model_entry(entry: dict[str, Any], set_active: bool = True) -> None:
    payload = json.dumps({"entry": entry, "set_active": bool(set_active)}, default=str)
    # One line, flushed: the parent reads line by line, and a stage that is
    # killed before its buffer drains would otherwise lose the registration.
    print(f"{MARKER} {payload}", flush=True)
