"""Document ingest and PDF-derived reads."""

from __future__ import annotations

import pytest

from document_service import (
    ingest_workspace_upload,
    read_workspace_file_for_llm,
    structured_derived_path,
)
from pdf_extraction import pymupdf_available
from workspace_service import read_text_file


@pytest.fixture
def isolated_workspace(tmp_path, monkeypatch):
    root = tmp_path / "workspaces"
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(root))
    monkeypatch.delenv("AGENT_PLATFORM_DB_PATH", raising=False)
    return root


def test_ingest_text_file(isolated_workspace):
    data = "# CV\n\nJane Doe — engineer".encode("utf-8")
    r = ingest_workspace_upload(1, filename="cv.md", data=data)
    assert r.path == "documents/cv.md"
    assert r.derived_path is None
    assert "Jane Doe" in r.excerpt
    assert read_text_file(1, "documents/cv.md") == data.decode()


@pytest.mark.skipif(not pymupdf_available(), reason="pymupdf not installed")
def test_ingest_pdf_creates_derived(isolated_workspace):
    import fitz

    doc = fitz.open()
    page = doc.new_page()
    page.insert_text((72, 72), "Senior Software Engineer")
    pdf_bytes = doc.tobytes()
    doc.close()

    r = ingest_workspace_upload(1, filename="resume.pdf", data=pdf_bytes)
    assert r.path == "documents/resume.pdf"
    assert r.derived_path == structured_derived_path("documents/resume.pdf")
    assert r.page_count == 1
    assert "Senior Software Engineer" in r.excerpt

    payload = read_workspace_file_for_llm(1, "documents/resume.pdf")
    assert payload["content_kind"] == "pdf_derived_markdown"
    assert "Senior Software Engineer" in payload["content"]


def test_workspace_upload_route(client, isolated_workspace, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(isolated_workspace))

    pr = c.post("/api/v1/projects/", json={"name": "Doc Test"})
    assert pr.status_code in (200, 201)
    pid = pr.json()["id"]

    files = {"file": ("notes.txt", b"hello workspace", "text/plain")}
    r = c.post(f"/api/v1/projects/{pid}/workspace/upload", files=files)
    assert r.status_code == 200
    body = r.json()
    assert body["path"] == "documents/notes.txt"
    assert "hello workspace" in body["excerpt"]

    r2 = c.get(f"/api/v1/projects/{pid}/workspace/file", params={"path": "documents/notes.txt"})
    assert r2.status_code == 200
    assert r2.json()["content"] == "hello workspace"
