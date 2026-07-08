"""Tool execution for the Coder agent: executor protocol + local implementation.

The ``ToolExecutor`` boundary is deliberately explicit so a future
``RemoteExecutor`` can proxy the same calls over a WebSocket to a thin runner
daemon on another machine (cloud backend + local hands). Nothing in the agent
loop may touch the filesystem directly — all effects go through an executor.
"""

from __future__ import annotations

import asyncio
import subprocess
from pathlib import Path
from typing import Any, Protocol

MAX_READ_BYTES = 256 * 1024
MAX_DIR_ENTRIES = 500

# OpenAI-format tool specs; forwarded verbatim through the LLM proxy so they
# work with any OpenAI-compatible provider (Ollama, Gemini, LM Studio, ...).
TOOL_SPECS: list[dict[str, Any]] = [
    {
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Read a text file from the workspace. Path is relative to the workspace root.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative file path, e.g. 'src/app.py'"},
                },
                "required": ["path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "write_file",
            "description": "Create or overwrite a text file in the workspace. Parent directories are created automatically.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative file path"},
                    "content": {"type": "string", "description": "Full new file content"},
                },
                "required": ["path", "content"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "list_dir",
            "description": "List entries in a workspace directory. Directories end with '/'.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative directory path; omit or '.' for the root"},
                },
                "required": [],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "run_command",
            "description": "Run a shell command in the workspace root and return stdout/stderr. Only available when command execution is enabled for the session.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command, e.g. 'pytest -q'"},
                },
                "required": ["command"],
            },
        },
    },
]


APPROVAL_REQUIRED_TOOLS = {"run_command"}


class ToolExecutionError(Exception):
    """A tool failure the model should see verbatim and recover from."""


class ToolExecutor(Protocol):
    async def execute(
        self, tool: str, args: dict[str, Any], *, call_id: str = ""
    ) -> str:
        """Run one tool call and return its textual result."""
        ...


class LocalExecutor:
    """Executes tools directly on this machine, jailed to one workspace root.

    Every path argument is resolved and verified to stay inside the root, so a
    model-supplied '../../etc/passwd' (or an absolute path, or a symlink that
    escapes) is rejected before any I/O happens.
    """

    def __init__(
        self,
        workspace_root: str,
        *,
        allow_commands: bool = False,
        command_timeout_seconds: float = 60.0,
    ) -> None:
        root = Path(workspace_root).expanduser().resolve()
        if not root.is_dir():
            raise ToolExecutionError(f"Workspace root is not a directory: {workspace_root}")
        self._root = root
        self._allow_commands = allow_commands
        self._command_timeout = command_timeout_seconds

    @property
    def workspace_root(self) -> Path:
        return self._root

    def _resolve(self, rel_path: str) -> Path:
        raw = (rel_path or "").strip() or "."
        candidate = Path(raw)
        p = (candidate if candidate.is_absolute() else self._root / candidate).resolve()
        try:
            p.relative_to(self._root)
        except ValueError:
            raise ToolExecutionError(
                f"Path escapes the workspace root and was blocked: {rel_path}"
            ) from None
        return p

    async def execute(
        self, tool: str, args: dict[str, Any], *, call_id: str = ""
    ) -> str:
        try:
            if tool == "read_file":
                return self._read_file(str(args.get("path", "")))
            if tool == "write_file":
                return self._write_file(str(args.get("path", "")), str(args.get("content", "")))
            if tool == "list_dir":
                return self._list_dir(str(args.get("path", ".")))
            if tool == "run_command":
                return await self._run_command(str(args.get("command", "")))
            return f"Error: unknown tool '{tool}'."
        except ToolExecutionError as e:
            # Returned as the tool result (not raised) so the model can correct itself.
            return f"Error: {e}"
        except OSError as e:
            return f"Error: {e}"

    def _read_file(self, rel_path: str) -> str:
        p = self._resolve(rel_path)
        if not p.is_file():
            raise ToolExecutionError(f"File not found: {rel_path}")
        data = p.read_bytes()
        truncated = len(data) > MAX_READ_BYTES
        text = data[:MAX_READ_BYTES].decode("utf-8", errors="replace")
        if truncated:
            text += f"\n...[truncated: file is {len(data)} bytes]"
        return text

    def _write_file(self, rel_path: str, content: str) -> str:
        if not (rel_path or "").strip():
            raise ToolExecutionError("write_file requires a non-empty path")
        p = self._resolve(rel_path)
        if p == self._root or p.is_dir():
            raise ToolExecutionError(f"Path is a directory, not a file: {rel_path}")
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(content.encode("utf-8"))
        return f"Wrote {len(content.encode('utf-8'))} bytes to {rel_path}"

    def _list_dir(self, rel_path: str) -> str:
        p = self._resolve(rel_path)
        if not p.is_dir():
            raise ToolExecutionError(f"Directory not found: {rel_path}")
        entries: list[str] = []
        for child in sorted(p.iterdir(), key=lambda c: (not c.is_dir(), c.name.lower())):
            entries.append(child.name + "/" if child.is_dir() else child.name)
            if len(entries) >= MAX_DIR_ENTRIES:
                entries.append(f"...[truncated at {MAX_DIR_ENTRIES} entries]")
                break
        return "\n".join(entries) if entries else "(empty directory)"

    async def _run_command(self, command: str) -> str:
        if not self._allow_commands:
            return (
                "Error: command execution is disabled for this session. "
                "Ask the user to enable it (allow_commands) if a command is required."
            )
        if not command.strip():
            raise ToolExecutionError("run_command requires a non-empty command")

        def _run() -> str:
            try:
                r = subprocess.run(
                    command,
                    shell=True,
                    cwd=self._root,
                    capture_output=True,
                    text=True,
                    timeout=self._command_timeout,
                )
            except subprocess.TimeoutExpired:
                return f"Error: command timed out after {self._command_timeout:.0f}s"
            out = (r.stdout or "") + (r.stderr or "")
            return f"[exit code {r.returncode}]\n{out}".strip()

        return await asyncio.to_thread(_run)


def make_executor(
    workspace_root: str,
    *,
    thread_id: int,
    client_id: str | None,
    allow_commands: bool,
    delegate_tools: bool = False,
) -> ToolExecutor:
    """Pick a tool executor for this client and workspace."""
    from coder.desktop_executor import (
        DesktopDelegatedExecutor,
        is_portal_desktop_client,
    )

    if is_portal_desktop_client(client_id) or delegate_tools:
        return DesktopDelegatedExecutor(
            thread_id=thread_id,
            workspace_root=workspace_root,
            allow_commands=allow_commands,
        )
    return LocalExecutor(workspace_root, allow_commands=allow_commands)
