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
from typing import Any, Iterator, Protocol

MAX_READ_BYTES = 256 * 1024
MAX_DIR_ENTRIES = 500

# --- search / repo_map limits -------------------------------------------------
# Mirrored verbatim in `desktop/crates/app/src/coder_tools.rs`; the desktop runs
# these tools when the Coder screen delegates, and a model that sees one shape
# locally and another remotely is being asked to learn two tools.

# Past this a file is a bundle, a lockfile or a data dump: a hit in one is never
# what was meant, and reading them is most of what a search costs.
SEARCH_MAX_FILE_BYTES = 1_000_000
SEARCH_MAX_HITS = 100
# One matching line, clipped — a minified line under the size cap would
# otherwise be the whole result.
SEARCH_MAX_HIT_CHARS = 300
MAP_MAX_FILES = 400
# Tooling state and build output. Dot-directories are skipped by rule (.git,
# .venv, .hearth); dot *files* are project config someone may well be after.
SKIP_DIRS = frozenset(
    {"node_modules", "target", "dist", "build", "__pycache__", "venv", "site-packages"}
)

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
            "name": "search",
            "description": (
                "Find which files contain a literal string, case-insensitively. "
                "Use this to locate code instead of reading files one at a time."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Literal text to find, e.g. 'def send_message'"},
                },
                "required": ["query"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "repo_map",
            "description": (
                "List the top-level definitions of every source file in the workspace "
                "(Python, Rust, JavaScript/TypeScript). Use this to see what exists and "
                "where a name lives before reading anything."
            ),
            "parameters": {"type": "object", "properties": {}, "required": []},
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

# --- repo_map: what counts as a definition ------------------------------------
# A token walk rather than regexes, because the same walk has to exist in Rust
# (`coder_tools.rs`) and two regex dialects drifting apart is a map that says
# different things depending on where the agent is running.
#
# Only column 0 counts, in every language: a Rust `impl` block's methods and a
# Python class's methods are detail, and the map answers "where does this name
# live", not "what is in this file".
#
# ponytail: three languages. Add a row when the agent is asked to work in a
# fourth — Go and Java are one entry each.
_MAP_LANGUAGES: dict[str, tuple[frozenset[str], bool]] = {
    ".py": (frozenset({"def", "class"}), False),
    ".pyi": (frozenset({"def", "class"}), False),
    ".rs": (
        frozenset({"fn", "struct", "enum", "trait", "type", "const", "static", "mod", "macro_rules!"}),
        False,
    ),
    **{
        ext: (
            frozenset(
                {"function", "class", "const", "let", "var", "interface", "type", "enum"}
            ),
            True,  # JS/TS: only exported names, which are the ones importers use
        )
        for ext in (".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts")
    },
}

# Words between the line start and the keyword. `pub(crate)` is matched by prefix.
_MAP_MODIFIERS = frozenset({"pub", "async", "unsafe", "extern", "default", "declare", "abstract"})


def _identifier(token: str) -> str:
    """The leading identifier of a token — `main()` → `main`, `Foo<T>` → `Foo`."""
    out: list[str] = []
    for ch in token:
        if ch.isalnum() or ch in "_$":
            out.append(ch)
        else:
            break
    return "".join(out)


def definition_name(line: str, keywords: frozenset[str], require_export: bool) -> str | None:
    """The name a source line declares, or None if it declares nothing."""
    if not line or line[:1].isspace():
        return None
    tokens = line.split()
    if require_export:
        if not tokens or tokens[0] != "export":
            return None
        tokens = tokens[1:]
    while tokens and (tokens[0] in _MAP_MODIFIERS or tokens[0].startswith("pub(")):
        tokens = tokens[1:]
    if len(tokens) < 2:
        return None
    keyword = tokens[0].rstrip("*")  # `function*`
    if keyword not in keywords:
        return None
    return _identifier(tokens[1]) or None


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
            if tool == "search":
                return self._search(str(args.get("query", "")))
            if tool == "repo_map":
                return self._repo_map()
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

    def _walk_files(self) -> Iterator[Path]:
        """Every file under the root, in a stable order, skipping build output.

        Stable is the point: the caps below cut a search off part-way, so an
        arbitrary walk order would return a different hundred hits each call.
        """
        stack = [self._root]
        while stack:
            try:
                children = sorted(stack.pop().iterdir(), key=lambda c: c.name)
            except OSError:
                continue
            subdirs: list[Path] = []
            for child in children:
                if child.is_dir():
                    if child.name not in SKIP_DIRS and not child.name.startswith("."):
                        subdirs.append(child)
                elif child.is_file():
                    yield child
            # Reversed, because the stack pops from the end.
            stack.extend(reversed(subdirs))

    def _rel(self, path: Path) -> str:
        return path.relative_to(self._root).as_posix()

    def _search(self, query: str) -> str:
        needle = query.strip().lower()
        if not needle:
            raise ToolExecutionError("search requires a non-empty query")
        hits: list[str] = []
        truncated = False
        for path in self._walk_files():
            if len(hits) >= SEARCH_MAX_HITS:
                truncated = True
                break
            try:
                if path.stat().st_size > SEARCH_MAX_FILE_BYTES:
                    continue
                # A binary fails to decode here, which is the sniffer.
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            rel = self._rel(path)
            for i, line in enumerate(text.splitlines(), start=1):
                if len(hits) >= SEARCH_MAX_HITS:
                    truncated = True
                    break
                if needle in line.lower():
                    hits.append(f"{rel}:{i}: {line.strip()[:SEARCH_MAX_HIT_CHARS]}")
        if not hits:
            return f"no matches for {query.strip()!r}"
        if truncated:
            hits.append(f"...[truncated at {SEARCH_MAX_HITS} matches — narrow the query]")
        return "\n".join(hits)

    def _repo_map(self) -> str:
        lines: list[str] = []
        scanned = 0
        truncated = False
        for path in self._walk_files():
            lang = _MAP_LANGUAGES.get(path.suffix.lower())
            if lang is None:
                continue
            if scanned >= MAP_MAX_FILES:
                truncated = True
                break
            scanned += 1
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            keywords, require_export = lang
            names: list[str] = []
            for line in text.splitlines():
                name = definition_name(line, keywords, require_export)
                if name and name not in names:
                    names.append(name)
            # A file with nothing top-level is dropped rather than listed empty:
            # list_dir already says what exists, and this answers a different
            # question — where a given name lives.
            if names:
                lines.append(f"{self._rel(path)}: {', '.join(names)}")
        if not lines:
            return (
                "no definitions found — this workspace may not be Python, Rust or "
                "JavaScript/TypeScript, or its code may be somewhere list_dir has not reached"
            )
        lines.sort()
        if truncated:
            lines.append(f"...[truncated at {MAP_MAX_FILES} files — use search instead]")
        return "\n".join(["definitions by file:", *lines])

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
