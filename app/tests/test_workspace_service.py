"""Tests for project sandbox path safety and I/O."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from workspace_service import (
    WorkspaceError,
    delete_path,
    delete_project_workspace,
    ensure_dir_path,
    ensure_project_dir,
    list_dir,
    normalize_relative_path,
    read_text_file,
    write_text_file,
    workspace_root,
)


@pytest.fixture
def isolated_workspace(tmp_path, monkeypatch):
    root = tmp_path / "workspaces"
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(root))
    monkeypatch.delenv("AGENT_PLATFORM_DB_PATH", raising=False)
    return root


def test_normalize_rejects_parent_segments():
    with pytest.raises(WorkspaceError) as e:
        normalize_relative_path("../etc/passwd")
    assert e.value.code == "invalid_path"


def test_normalize_empty():
    assert normalize_relative_path("") == ""
    assert normalize_relative_path("  ") == ""


def test_write_read_roundtrip(isolated_workspace):
    write_text_file(1, "src/hello.txt", "hello")
    assert read_text_file(1, "src/hello.txt") == "hello"
    entries = list_dir(1, "src")
    assert any(e.name == "hello.txt" and e.kind == "file" for e in entries)


def test_cannot_escape_sandbox(isolated_workspace):
    ensure_project_dir(1)
    base = isolated_workspace / "project-1"
    evil = Path("../../outside")
    # Direct write outside via normalized path should fail
    with pytest.raises(WorkspaceError):
        write_text_file(1, "../outside.txt", "x")


def test_delete_file(isolated_workspace):
    write_text_file(1, "a.txt", "a")
    delete_path(1, "a.txt")
    with pytest.raises(WorkspaceError) as e:
        read_text_file(1, "a.txt")
    assert e.value.code == "not_found"


def test_delete_project_workspace_removes_tree(isolated_workspace):
    write_text_file(2, "x/y.txt", "z")
    delete_project_workspace(2)
    assert not (isolated_workspace / "project-2").exists()


def test_workspace_root_creates_directory(isolated_workspace):
    r = workspace_root()
    assert r == isolated_workspace.resolve()
    assert r.is_dir()


def test_ensure_dir_path_nested(isolated_workspace):
    p = ensure_dir_path(3, "processes/9/sub")
    assert p.is_dir()
    assert "processes" in str(p).replace("\\", "/")
