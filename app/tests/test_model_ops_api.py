"""Contract and unit tests for model-ops API."""

from __future__ import annotations

import json
import os
from unittest.mock import AsyncMock, patch

import pytest

pytestmark = pytest.mark.contract


@pytest.fixture
def model_ops_data_dir(tmp_path, monkeypatch):
    root = tmp_path / "model_ops"
    monkeypatch.setenv("MODEL_OPS_DATA_DIR", str(root))
    monkeypatch.setenv("MODEL_OPS_GPU_SUBPROCESS", "0")
    return root


def _master_headers(client):
    key = os.environ.get("AGENT_PLATFORM_MASTER_KEY", "test-master-key")
    return {"Authorization": f"Bearer {key}"}


def test_model_ops_scopes_listed(client):
    c, _, _ = client
    r = c.get("/api/v1/api-tokens/scopes", headers=_master_headers(c))
    assert r.status_code == 200
    scopes = {s["scope"] for s in r.json()["scopes"]}
    assert "model:read" in scopes
    assert "model:write" in scopes


def test_model_ops_create_project_and_list(client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    r = c.post(
        "/api/v1/model-ops/projects",
        headers=h,
        json={"name": "test-proj", "description": "demo", "ollama_tag": "test-proj"},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["name"] == "test-proj"
    assert "manifest" in body
    assert "id" in body

    r2 = c.get("/api/v1/model-ops/projects", headers=h)
    assert r2.status_code == 200
    projects = r2.json()["projects"]
    assert any(p["name"] == "test-proj" for p in projects)


def test_model_ops_get_project(client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    c.post("/api/v1/model-ops/projects", headers=h, json={"name": "get-me"})
    r = c.get("/api/v1/model-ops/projects/get-me", headers=h)
    assert r.status_code == 200
    assert r.json()["name"] == "get-me"
    assert "registry_entries" in r.json()


@patch("model_ops.ollama_client.list_models", new_callable=AsyncMock)
def test_model_ops_ollama_list_models(mock_list, client):
    c, _, _ = client
    mock_list.return_value = {"models": [{"name": "llama3.2:latest", "size": 123}]}
    r = c.get("/api/v1/model-ops/ollama/models", headers=_master_headers(c))
    assert r.status_code == 200
    assert "models" in r.json()
    assert r.json()["models"][0]["name"] == "llama3.2:latest"


def test_model_ops_registry_empty(client, model_ops_data_dir):
    c, _, _ = client
    r = c.get("/api/v1/model-ops/registry", headers=_master_headers(c))
    assert r.status_code == 200
    assert "entries" in r.json()
    assert isinstance(r.json()["entries"], list)


def test_model_build_operation_contract(client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    c.post("/api/v1/model-ops/projects", headers=h, json={"name": "op-proj"})

    with patch("model_ops.routes._run_job_background"):
        r = c.post(
            "/api/v1/model-ops/operations/build",
            headers=h,
            json={
                "operation": "model.build",
                "input": {
                    "project": "op-proj",
                    "stages": ["prepare"],
                    "offline_eval": True,
                },
            },
        )
    assert r.status_code == 200
    body = r.json()
    assert body["operation"] == "model.build"
    assert "job_id" in body
    assert body["poll_url"].endswith(f"/model-ops/jobs/{body['job_id']}")
    assert "/stream" in body["stream_url"]


def test_model_ops_job_envelope(client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    c.post("/api/v1/model-ops/projects", headers=h, json={"name": "job-proj"})

    with patch("model_ops.routes._run_job_background"):
        r = c.post(
            "/api/v1/model-ops/jobs",
            headers=h,
            json={"project": "job-proj", "stages": ["prepare"], "offline_eval": True},
        )
    assert r.status_code == 200
    job = r.json()
    for key in ("id", "status", "stages", "poll_url", "stream_url", "project_name"):
        assert key in job
    assert job["status"] == "pending"
    assert job["project_name"] == "job-proj"

    r2 = c.get(f"/api/v1/model-ops/jobs/{job['id']}", headers=h)
    assert r2.status_code == 200
    assert r2.json()["id"] == job["id"]



def test_openapi_includes_model_ops_tag(client):
    c, _, _ = client
    r = c.get("/openapi.json")
    assert r.status_code == 200
    spec = r.json()
    tags = {t["name"] for t in spec.get("tags", [])}
    paths = spec.get("paths", {})
    has_model_ops = any("/model-ops/" in p for p in paths)
    assert has_model_ops or "model-ops" in tags


@patch("model_ops.routes._run_job_background")
def test_model_ops_async_pull_job(mock_bg, client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    r = c.post(
        "/api/v1/model-ops/ollama/models/pull",
        headers=h,
        json={"name": "llama3.2:latest", "async": True},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["job_type"] == "ollama_pull"
    assert body["status"] == "pending"
    assert body["project_id"] is None
    assert "poll_url" in body
    mock_bg.assert_called_once()


@patch("model_ops.routes._run_job_background")
def test_model_ops_ollama_copy_job(mock_bg, client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    r = c.post(
        "/api/v1/model-ops/ollama/models/copy",
        headers=h,
        json={"source": "a:latest", "destination": "b:latest", "async": True},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["job_type"] == "ollama_copy"
    mock_bg.assert_called_once()


@patch("model_ops.routes._run_job_background")
def test_model_ops_process_link(mock_bg, client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    c.post("/api/v1/model-ops/projects", headers=h, json={"name": "link-proj"})

    teams = c.get("/api/v1/teams/", headers=h)
    assert teams.status_code == 200
    tid = teams.json()["teams"][0]["id"]

    proc = c.post("/api/v1/processes", headers=h, json={"goal": "Train a model", "team_template_id": tid})
    assert proc.status_code == 200
    process_id = proc.json()["process_id"]

    r = c.post(
        "/api/v1/model-ops/jobs",
        headers=h,
        json={"project": "link-proj", "stages": ["prepare"], "process_id": process_id},
    )
    assert r.status_code == 200
    job_id = r.json()["id"]

    detail = c.get(f"/api/v1/processes/{process_id}", headers=h)
    assert detail.status_code == 200
    assert detail.json()["process"]["model_build_job_id"] == job_id


def test_model_ops_prepare_stage_integration(client, model_ops_data_dir):
    c, _, _ = client
    h = _master_headers(c)
    c.post("/api/v1/model-ops/projects", headers=h, json={"name": "prep-proj"})

    example = {
        "messages": [
            {"role": "user", "content": json.dumps({"context": "hello world"})},
            {"role": "assistant", "content": "Hi there"},
        ]
    }
    example2 = {
        "messages": [
            {"role": "user", "content": json.dumps({"context": "second example"})},
            {"role": "assistant", "content": "Sure"},
        ]
    }
    knowledge = (json.dumps(example) + "\n" + json.dumps(example2) + "\n").encode("utf-8")
    files = [("files", ("examples.jsonl", knowledge, "application/jsonl"))]

    up = c.post(
        "/api/v1/model-ops/projects/prep-proj/knowledge",
        headers=h,
        files=files,
    )
    assert up.status_code == 200

    from model_ops.pipeline import build_dataset, merge_knowledge

    merge_knowledge.merge_packs("prep-proj")
    train_path, eval_path = build_dataset.build_dataset("prep-proj")

    assert train_path.exists()
    assert eval_path.exists()
    lines = train_path.read_text(encoding="utf-8").strip().splitlines()
    assert len(lines) >= 1



def test_job_log_reads_are_incremental_and_windowed(tmp_path):
    """SSE log streaming must slice by byte offset, not by tail-string length."""
    from types import SimpleNamespace

    from model_ops import service
    from model_ops.service import read_job_log_since, read_job_log_tail

    log = tmp_path / "job.log"
    log.write_text("".join(f"line {i}\n" for i in range(500)), encoding="utf-8")
    job = SimpleNamespace(log_path=str(log))

    # Tail keeps only the last N lines, and never reads the whole file.
    tail = read_job_log_tail(job, lines=200).splitlines()
    assert len(tail) == 200
    assert tail[-1] == "line 499"

    # Streaming from the end sees nothing until the writer appends.
    offset = log.stat().st_size
    chunk, offset = read_job_log_since(job, offset)
    assert chunk == ""

    with log.open("a", encoding="utf-8") as f:
        f.write("line 500\n")
    chunk, offset = read_job_log_since(job, offset)
    assert chunk.rstrip("\r\n") == "line 500"  # text-mode writers emit CRLF on Windows
    assert offset == log.stat().st_size

    # Second poll with no writes must not re-send anything (the old length-slice bug).
    chunk, offset = read_job_log_since(job, offset)
    assert chunk == ""

    # A truncated/rotated log restarts from 0 instead of returning garbage.
    log.write_text("fresh\n", encoding="utf-8")
    chunk, offset = read_job_log_since(job, offset)
    assert chunk.rstrip("\r\n") == "fresh"

    # A log larger than the tail window drops the half-line the window cut.
    log.write_text("x" * (service._LOG_TAIL_BYTES + 10) + "\ntail line\n", encoding="utf-8")
    windowed = read_job_log_tail(job, lines=200).splitlines()
    assert windowed[-1] == "tail line"
    assert len(windowed) == 1
