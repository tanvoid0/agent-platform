"""Per-project file sandbox HTTP API."""

from __future__ import annotations

from fastapi import APIRouter, Depends, File, HTTPException, UploadFile
from pydantic import BaseModel, Field
from sqlmodel import Session

from api_tokens.auth import TokenPrincipal, assert_token_project_access, require_valid_token
from database import get_session
from document_service import ingest_workspace_upload, read_workspace_file_for_llm
from models import Process, Project
from workspace_service import (
    WorkspaceError,
    delete_path,
    ensure_dir_path,
    ensure_process_workspace,
    list_dir,
    mkdir,
    normalize_relative_path,
    process_workspace_rel,
    read_text_file,
    write_text_file,
)

router = APIRouter(prefix="/projects/{project_id}/workspace", tags=["workspace"])


def _require_project(session: Session, project_id: int, principal: TokenPrincipal) -> Project:
    row = session.get(Project, project_id)
    if not row:
        raise HTTPException(status_code=404, detail="Project not found")
    assert_token_project_access(principal, project_id)
    return row


def _require_process_for_project(session: Session, project_id: int, process_id: int) -> Process:
    proc = session.get(Process, process_id)
    if not proc:
        raise HTTPException(status_code=404, detail="Process not found")
    if proc.project_id != project_id:
        raise HTTPException(status_code=404, detail="Process does not belong to this project")
    return proc


class FileWriteBody(BaseModel):
    path: str = Field(min_length=1, max_length=8192)
    content: str = Field(default="")


class MkdirBody(BaseModel):
    path: str = Field(min_length=1, max_length=8192)


class EnsureProcessBody(BaseModel):
    process_id: int = Field(ge=1)


def _map_error(e: WorkspaceError) -> HTTPException:
    return HTTPException(status_code=e.http_status, detail=f"{e.code}: {e.message}")


@router.get("/info")
def workspace_info(
    project_id: int,
    path: str = "",
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """
    Absolute path on the agent-platform server for a folder inside the project sandbox.
    Pass `path` as the same relative path used for /workspace/list (e.g. empty for project root,
    or processes/<process_id>/ for a run). Creates the directory tree if missing. Use this path
    in Explorer / Finder / a terminal on the machine where the API stores files (often the server).
    """
    _require_project(session, project_id, principal)
    n = normalize_relative_path(path)
    if n.startswith("processes/"):
        seg = n.split("/", 2)
        if len(seg) >= 2 and seg[1].isdigit():
            _require_process_for_project(session, project_id, int(seg[1]))
    try:
        p = ensure_dir_path(project_id, path)
        return {
            "absolute_path": str(p),
            "relative_prefix": n,
        }
    except WorkspaceError as e:
        raise _map_error(e) from e


@router.post("/ensure-process", status_code=201)
def workspace_ensure_process(
    project_id: int,
    body: EnsureProcessBody,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Create processes/{process_id}/ if missing (after validating the process belongs to the project)."""
    _require_project(session, project_id, principal)
    _require_process_for_project(session, project_id, body.process_id)
    try:
        p = ensure_process_workspace(project_id, body.process_id)
        return {
            "ok": True,
            "absolute_path": str(p),
            "relative_prefix": process_workspace_rel(body.process_id),
        }
    except WorkspaceError as e:
        raise _map_error(e) from e


@router.get("/list")
def workspace_list(
    project_id: int,
    path: str = "",
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_project(session, project_id, principal)
    try:
        entries = list_dir(project_id, path)
        return {
            "entries": [{"name": e.name, "path": e.path, "type": e.kind} for e in entries],
        }
    except WorkspaceError as e:
        raise _map_error(e) from e


@router.get("/file")
def workspace_read_file(
    project_id: int,
    path: str,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_project(session, project_id, principal)
    try:
        payload = read_workspace_file_for_llm(project_id, path)
        return payload
    except WorkspaceError as e:
        raise _map_error(e) from e


@router.post("/upload")
async def workspace_upload_file(
    project_id: int,
    file: UploadFile = File(...),
    dest: str = "documents",
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """Multipart upload; PDFs are extracted to ``<path>.derived/structured.md``."""
    _require_project(session, project_id, principal)
    name = (file.filename or "upload").strip()
    try:
        data = await file.read()
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"read_failed: {e}") from e
    try:
        result = ingest_workspace_upload(
            project_id,
            filename=name,
            data=data,
            dest_dir=dest,
        )
    except WorkspaceError as e:
        raise _map_error(e) from e
    return {
        "path": result.path,
        "mime_type": result.mime_type,
        "bytes": result.bytes_written,
        "derived_path": result.derived_path,
        "manifest_path": result.manifest_path,
        "page_count": result.page_count,
        "excerpt": result.excerpt,
        "extraction": result.extraction,
    }


@router.put("/file")
def workspace_write_file(
    project_id: int,
    body: FileWriteBody,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_project(session, project_id, principal)
    try:
        write_text_file(project_id, body.path, body.content)
        return {"ok": True, "path": body.path}
    except WorkspaceError as e:
        raise _map_error(e) from e


@router.delete("/file")
def workspace_delete_file(
    project_id: int,
    path: str,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_project(session, project_id, principal)
    try:
        delete_path(project_id, path)
        return {"ok": True}
    except WorkspaceError as e:
        raise _map_error(e) from e


@router.post("/mkdir", status_code=201)
def workspace_mkdir(
    project_id: int,
    body: MkdirBody,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    _require_project(session, project_id, principal)
    try:
        mkdir(project_id, body.path)
        return {"ok": True, "path": body.path}
    except WorkspaceError as e:
        raise _map_error(e) from e
