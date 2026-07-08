"""Delegate coder tool execution to Portal Desktop when the platform runs remotely."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

from coder.executor import ToolExecutionError

PORTAL_DESKTOP_CLIENT_ID = "portal-desktop"


def is_portal_desktop_client(client_id: str | None) -> bool:
    return (client_id or "").strip().lower() == PORTAL_DESKTOP_CLIENT_ID


# (thread_id, call_id) -> Future[str]
_pending: dict[tuple[int, str], asyncio.Future[str]] = {}


def resolve_desktop_tool_result(thread_id: int, call_id: str, result: str) -> None:
    key = (thread_id, call_id)
    fut = _pending.get(key)
    if fut is None or fut.done():
        raise KeyError(
            f"No pending desktop tool call for thread={thread_id} call_id={call_id!r}"
        )
    fut.set_result(result)


class DesktopDelegatedExecutor:
    """Workspace tools run on the desktop host; the platform waits for results."""

    def __init__(
        self,
        *,
        thread_id: int,
        workspace_root: str,
        allow_commands: bool = False,
    ) -> None:
        root = (workspace_root or "").strip()
        if not root:
            raise ToolExecutionError(
                "workspace_root is required for desktop-delegated execution"
            )
        self._thread_id = thread_id
        self._root = root
        self._allow_commands = allow_commands

    @property
    def workspace_root(self) -> Path:
        return Path(self._root)

    async def execute(self, tool: str, args: dict[str, Any], *, call_id: str = "") -> str:
        if not call_id:
            return "Error: internal error: missing call_id for desktop tool delegation"
        loop = asyncio.get_running_loop()
        fut: asyncio.Future[str] = loop.create_future()
        key = (self._thread_id, call_id)
        if key in _pending:
            return f"Error: duplicate tool call id {call_id}"
        _pending[key] = fut
        try:
            return await asyncio.wait_for(fut, timeout=300.0)
        except TimeoutError:
            return "Error: timed out waiting for desktop to execute tool"
        finally:
            _pending.pop(key, None)
