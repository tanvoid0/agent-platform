"""Execution context for LLM tool calls (DAG / process-scoped)."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class ToolContext:
    """
    Passed from DAGExecutor into tool handlers; never trust model-supplied project ids for workspace I/O.

    `process_id` is the orchestration run id; with `project_id`, workspace_* tools use folder
    processes/<process_id>/ under that project.
    """

    process_id: int | None = None
    project_id: int | None = None
