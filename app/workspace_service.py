"""
Per-project sandbox directories under AGENT_PLATFORM_WORKSPACE_ROOT (or default next to DB data).

Used by REST routes and DAG tool handlers — keep all path logic here.

Future hardening (optional): a SQLModel table indexing path, size, and updated_at for quotas,
search, and audit without full directory walks — keep the service API stable if added.
"""

from __future__ import annotations

import os
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Literal


class WorkspaceError(Exception):
    """Business-rule or path-safety failure (map to HTTP 400/404/413)."""

    def __init__(self, code: str, message: str, http_status: int = 400):
        self.code = code
        self.message = message
        self.http_status = http_status
        super().__init__(message)


def _default_workspace_root() -> Path:
    raw = (os.getenv("AGENT_PLATFORM_DB_PATH") or "data/agent_platform.db").strip()
    p = Path(raw)
    parent = p.parent
    if parent == Path(".") or str(parent) == "":
        parent = Path("data")
    return parent / "workspaces"


def workspace_root() -> Path:
    """Resolved absolute path; directory is created if missing."""
    env = (os.getenv("AGENT_PLATFORM_WORKSPACE_ROOT") or "").strip()
    root = Path(env).expanduser() if env else _default_workspace_root()
    root = root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    return root


def project_sandbox_dir(project_id: int) -> Path:
    if project_id < 1:
        raise WorkspaceError("invalid_project", "project_id must be positive", 400)
    d = workspace_root() / f"project-{project_id}"
    return d


def ensure_project_dir(project_id: int) -> Path:
    d = project_sandbox_dir(project_id)
    d.mkdir(parents=True, exist_ok=True)
    return d.resolve()


MAX_PATH_SEGMENTS = 32
MAX_FILE_BYTES = int((os.getenv("AGENT_PLATFORM_WORKSPACE_MAX_FILE_BYTES") or str(8 * 1024 * 1024)).strip() or str(8 * 1024 * 1024))


def normalize_relative_path(rel: str) -> str:
    """
    Return a '/'-separated relative path with no '..' or absolute segments.
    Empty string means sandbox root.
    """
    if rel is None:
        return ""
    s = rel.replace("\\", "/").strip()
    if not s or s == ".":
        return ""
    parts: list[str] = []
    for seg in s.strip("/").split("/"):
        if seg in ("", "."):
            continue
        if seg == "..":
            raise WorkspaceError("invalid_path", "Path must not contain '..'")
        if len(seg) > 255:
            raise WorkspaceError("invalid_path", "Path segment too long")
        parts.append(seg)
    if len(parts) > MAX_PATH_SEGMENTS:
        raise WorkspaceError("invalid_path", f"Path exceeds {MAX_PATH_SEGMENTS} segments")
    return "/".join(parts)


def _resolve_under_project(project_id: int, rel: str) -> Path:
    """Absolute path inside project sandbox; must exist for read/delete."""
    base = ensure_project_dir(project_id)
    n = normalize_relative_path(rel)
    target = (base / n.replace("/", os.sep)) if n else base
    try:
        resolved = target.resolve()
    except OSError as e:
        raise WorkspaceError("invalid_path", str(e)) from e
    base_resolved = base.resolve()
    if resolved == base_resolved:
        return resolved
    try:
        resolved.relative_to(base_resolved)
    except ValueError as e:
        raise WorkspaceError("invalid_path", "Path escapes sandbox") from e
    return resolved


def _resolve_under_project_for_write(project_id: int, rel: str) -> Path:
    """Like _resolve_under_project but for new files (parent dirs must exist or be creatable)."""
    base = ensure_project_dir(project_id)
    n = normalize_relative_path(rel)
    if not n:
        raise WorkspaceError("invalid_path", "File path must not be empty")
    target = base / n.replace("/", os.sep)
    try:
        resolved = target.resolve()
    except OSError as e:
        raise WorkspaceError("invalid_path", str(e)) from e
    base_resolved = base.resolve()
    try:
        resolved.relative_to(base_resolved)
    except ValueError as e:
        raise WorkspaceError("invalid_path", "Path escapes sandbox") from e
    return resolved


@dataclass(frozen=True)
class DirEntry:
    name: str
    path: str  # relative posix path under sandbox
    kind: Literal["file", "dir"]


def list_dir(project_id: int, rel: str = "") -> list[DirEntry]:
    """
    List children of a directory relative to project root.
    `rel` empty is the project root.
    """
    base = ensure_project_dir(project_id)
    n = normalize_relative_path(rel)
    target = (base / n.replace("/", os.sep)) if n else base
    try:
        resolved = target.resolve()
    except OSError as e:
        raise WorkspaceError("invalid_path", str(e)) from e
    base_resolved = base.resolve()
    if resolved != base_resolved:
        try:
            resolved.relative_to(base_resolved)
        except ValueError as e:
            raise WorkspaceError("invalid_path", "Path escapes sandbox") from e
    if not resolved.is_dir():
        raise WorkspaceError("not_a_directory", "Path is not a directory", 400)
    out: list[DirEntry] = []
    try:
        for item in sorted(resolved.iterdir(), key=lambda p: (not p.is_dir(), p.name.lower())):
            name = item.name
            if name.startswith("."):
                continue
            rel_path = f"{n}/{name}" if n else name
            if item.is_dir():
                out.append(DirEntry(name=name, path=rel_path.replace("\\", "/"), kind="dir"))
            elif item.is_file():
                out.append(DirEntry(name=name, path=rel_path.replace("\\", "/"), kind="file"))
    except OSError as e:
        raise WorkspaceError("io_error", str(e), 500) from e
    return out


def read_text_file(project_id: int, rel: str) -> str:
    path = _resolve_under_project_for_write(project_id, rel)
    if path.is_dir():
        raise WorkspaceError("is_directory", "Path is a directory", 400)
    if not path.is_file():
        raise WorkspaceError("not_found", "File not found", 404)
    size = path.stat().st_size
    if size > MAX_FILE_BYTES:
        raise WorkspaceError("file_too_large", f"File exceeds {MAX_FILE_BYTES} bytes", 413)
    raw = path.read_bytes()
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as e:
        raise WorkspaceError("not_utf8", "File is not valid UTF-8 text", 415) from e


def write_text_file(project_id: int, rel: str, content: str) -> None:
    path = _resolve_under_project_for_write(project_id, rel)
    if path.exists() and path.is_dir():
        raise WorkspaceError("is_directory", "Path is a directory", 400)
    data = content.encode("utf-8")
    if len(data) > MAX_FILE_BYTES:
        raise WorkspaceError("file_too_large", f"Content exceeds {MAX_FILE_BYTES} bytes", 413)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def delete_path(project_id: int, rel: str) -> None:
    n = normalize_relative_path(rel)
    if not n:
        raise WorkspaceError("invalid_path", "Cannot delete sandbox root", 400)
    path = _resolve_under_project(project_id, rel)
    base = ensure_project_dir(project_id)
    if path.resolve() == base.resolve():
        raise WorkspaceError("invalid_path", "Cannot delete sandbox root", 400)
    if not path.exists():
        raise WorkspaceError("not_found", "Path not found", 404)
    try:
        if path.is_dir():
            path.rmdir()  # only empty dirs
        else:
            path.unlink()
    except OSError as e:
        if path.is_dir():
            raise WorkspaceError("directory_not_empty", str(e), 400) from e
        raise WorkspaceError("io_error", str(e), 500) from e


def delete_project_workspace(project_id: int) -> None:
    """Remove all files for a project (best-effort)."""
    d = project_sandbox_dir(project_id)
    if d.is_dir():
        shutil.rmtree(d, ignore_errors=True)


def mkdir(project_id: int, rel: str) -> None:
    n = normalize_relative_path(rel)
    if not n:
        raise WorkspaceError("invalid_path", "Directory path must not be empty", 400)
    path = _resolve_under_project_for_write(project_id, n)
    path.mkdir(parents=True, exist_ok=True)


def process_workspace_rel(process_id: int) -> str:
    """Relative path under a project for one orchestration run (process)."""
    if process_id < 1:
        raise WorkspaceError("invalid_process", "process_id must be positive", 400)
    return f"processes/{process_id}"


def ensure_process_workspace(project_id: int, process_id: int) -> Path:
    """Create processes/{process_id}/ and return its absolute path."""
    rel = process_workspace_rel(process_id)
    mkdir(project_id, rel)
    base = ensure_project_dir(project_id)
    return (base / rel.replace("/", os.sep)).resolve()


def ensure_dir_path(project_id: int, rel: str) -> Path:
    """
    Ensure `rel` exists as a directory under the project sandbox (mkdir parents) and return absolute path.
    Empty `rel` is the project root.
    """
    n = normalize_relative_path(rel)
    if not n:
        return ensure_project_dir(project_id)
    mkdir(project_id, n)
    base = ensure_project_dir(project_id)
    target = (base / n.replace("/", os.sep)).resolve()
    base_resolved = base.resolve()
    try:
        target.relative_to(base_resolved)
    except ValueError as e:
        raise WorkspaceError("invalid_path", "Path escapes sandbox") from e
    return target
