"""
Ingest workspace documents (PDF, text) and produce LLM-readable derived views.
"""

from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from pdf_extraction import extract_pdf_structured_markdown, pymupdf_available
from workspace_service import (
    MAX_FILE_BYTES,
    WorkspaceError,
    normalize_relative_path,
    read_text_file,
    write_text_file,
    _resolve_under_project_for_write,
    ensure_project_dir,
)

ALLOWED_UPLOAD_SUFFIXES = frozenset({".pdf", ".txt", ".md", ".markdown"})
PDF_SUFFIX = ".pdf"
DERIVED_DIR_NAME = ".derived"
STRUCTURED_MD = "structured.md"
MANIFEST_JSON = "manifest.json"
MAX_CHAT_EXCERPT_CHARS = 24_000


@dataclass(frozen=True)
class DocumentIngestResult:
    path: str
    mime_type: str
    bytes_written: int
    derived_path: str | None
    manifest_path: str | None
    page_count: int | None
    excerpt: str
    extraction: str


def derived_prefix_for_file(rel: str) -> str:
    n = normalize_relative_path(rel)
    return f"{n}{DERIVED_DIR_NAME}"


def structured_derived_path(rel: str) -> str:
    return f"{derived_prefix_for_file(rel)}/{STRUCTURED_MD}"


def manifest_derived_path(rel: str) -> str:
    return f"{derived_prefix_for_file(rel)}/{MANIFEST_JSON}"


def _safe_upload_basename(name: str) -> str:
    base = Path(name).name.strip()
    if not base or base in (".", ".."):
        raise WorkspaceError("invalid_filename", "Filename is required", 400)
    base = re.sub(r"[^\w.\- ]+", "_", base).strip()
    if not base:
        raise WorkspaceError("invalid_filename", "Filename is invalid", 400)
    return base


def _mime_for_suffix(suffix: str) -> str:
    s = suffix.lower()
    if s == PDF_SUFFIX:
        return "application/pdf"
    if s in (".md", ".markdown"):
        return "text/markdown"
    return "text/plain"


def ingest_workspace_upload(
    project_id: int,
    *,
    filename: str,
    data: bytes,
    dest_dir: str = "documents",
) -> DocumentIngestResult:
    if len(data) > MAX_FILE_BYTES:
        raise WorkspaceError("file_too_large", f"File exceeds {MAX_FILE_BYTES} bytes", 413)
    if not data:
        raise WorkspaceError("empty_file", "File is empty", 400)

    safe_name = _safe_upload_basename(filename)
    suffix = Path(safe_name).suffix.lower()
    if suffix not in ALLOWED_UPLOAD_SUFFIXES:
        raise WorkspaceError(
            "unsupported_type",
            f"Supported uploads: {', '.join(sorted(ALLOWED_UPLOAD_SUFFIXES))}",
            415,
        )

    dir_rel = normalize_relative_path(dest_dir) or "documents"
    rel = f"{dir_rel}/{safe_name}" if dir_rel else safe_name
    rel = normalize_relative_path(rel)

    path = _resolve_under_project_for_write(project_id, rel)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)

    mime = _mime_for_suffix(suffix)
    if suffix == PDF_SUFFIX:
        return _ingest_pdf(project_id, rel, data, mime)
    text = data.decode("utf-8")
    excerpt = _clip_excerpt(text)
    return DocumentIngestResult(
        path=rel,
        mime_type=mime,
        bytes_written=len(data),
        derived_path=None,
        manifest_path=None,
        page_count=None,
        excerpt=excerpt,
        extraction="utf8",
    )


def _ingest_pdf(project_id: int, rel: str, data: bytes, mime: str) -> DocumentIngestResult:
    structured_rel = structured_derived_path(rel)
    manifest_rel = manifest_derived_path(rel)
    extraction = "pymupdf"
    page_count: int | None = None
    error: str | None = None
    markdown = ""

    try:
        result = extract_pdf_structured_markdown(data)
        markdown = result.markdown
        page_count = result.page_count
        extraction = result.extractor
    except RuntimeError as e:
        error = str(e)
        markdown = (
            f"# PDF: {Path(rel).name}\n\n"
            f"_Extraction failed: {error}_\n\n"
            "The original PDF was saved in the workspace. "
            "Install `pymupdf` on the server or paste text into chat."
        )
        extraction = "failed"

    write_text_file(project_id, structured_rel, markdown)
    manifest = {
        "source_path": rel,
        "derived_structured": structured_rel,
        "mime_type": mime,
        "page_count": page_count,
        "extractor": extraction,
        "pymupdf_available": pymupdf_available(),
        "error": error,
        "updated_at": datetime.now(timezone.utc).isoformat(),
    }
    write_text_file(project_id, manifest_rel, json.dumps(manifest, indent=2))

    excerpt = _clip_excerpt(markdown)
    return DocumentIngestResult(
        path=rel,
        mime_type=mime,
        bytes_written=len(data),
        derived_path=structured_rel,
        manifest_path=manifest_rel,
        page_count=page_count,
        excerpt=excerpt,
        extraction=extraction,
    )


def _clip_excerpt(text: str) -> str:
    t = text.strip()
    if len(t) <= MAX_CHAT_EXCERPT_CHARS:
        return t
    return (
        t[: MAX_CHAT_EXCERPT_CHARS - 80].rstrip()
        + "\n\n---\n_(Excerpt truncated for chat; use workspace_read on the derived path for full text.)_"
    )


def read_workspace_file_for_llm(project_id: int, rel: str) -> dict:
    """
    Read a workspace path for agents / API: UTF-8 text, or PDF via derived markdown.
    """
    n = normalize_relative_path(rel)
    if not n:
        raise WorkspaceError("invalid_path", "path is required", 400)

    if n.lower().endswith(PDF_SUFFIX):
        structured = structured_derived_path(n)
        base = ensure_project_dir(project_id)
        derived_file = (base / structured.replace("/", os.sep)).resolve()
        if derived_file.is_file():
            content = read_text_file(project_id, structured)
            return {
                "path": n,
                "content": content,
                "content_kind": "pdf_derived_markdown",
                "derived_path": structured,
            }
        raw_path = _resolve_under_project_for_write(project_id, n)
        if not raw_path.is_file():
            raise WorkspaceError("not_found", "File not found", 404)
        data = raw_path.read_bytes()
        if len(data) > MAX_FILE_BYTES:
            raise WorkspaceError("file_too_large", f"File exceeds {MAX_FILE_BYTES} bytes", 413)
        result = _ingest_pdf(project_id, n, data, "application/pdf")
        return {
            "path": n,
            "content": read_text_file(project_id, result.derived_path or structured),
            "content_kind": "pdf_derived_markdown",
            "derived_path": result.derived_path,
        }

    content = read_text_file(project_id, n)
    return {
        "path": n,
        "content": content,
        "content_kind": "text",
        "derived_path": None,
    }
