"""Tests for the Coder agent API: executor jail + tool-calling agent loop."""

import asyncio
import json

import pytest
from sqlmodel import Session, select

from coder.executor import LocalExecutor
from coder.models import CoderChatThread


def _run(coro):
    return asyncio.run(coro)


# ---------------------------------------------------------------------------
# LocalExecutor unit tests (the security boundary)
# ---------------------------------------------------------------------------


def test_executor_write_read_roundtrip(tmp_path):
    ex = LocalExecutor(str(tmp_path))
    out = _run(ex.execute("write_file", {"path": "pkg/mod.py", "content": "x = 1\n"}))
    assert "pkg/mod.py" in out
    assert (tmp_path / "pkg" / "mod.py").read_text(encoding="utf-8") == "x = 1\n"
    assert _run(ex.execute("read_file", {"path": "pkg/mod.py"})) == "x = 1\n"


def test_executor_blocks_path_escape(tmp_path):
    ex = LocalExecutor(str(tmp_path))
    for bad in ("../outside.txt", "..\\outside.txt", "a/../../outside.txt"):
        out = _run(ex.execute("write_file", {"path": bad, "content": "nope"}))
        assert out.startswith("Error:"), bad
        assert "workspace root" in out
    # Absolute path outside the root is blocked too.
    outside = tmp_path.parent / "outside.txt"
    out = _run(ex.execute("read_file", {"path": str(outside)}))
    assert out.startswith("Error:")
    assert not outside.exists()


def test_executor_list_dir_and_missing_file(tmp_path):
    (tmp_path / "sub").mkdir()
    (tmp_path / "a.txt").write_text("hi", encoding="utf-8")
    ex = LocalExecutor(str(tmp_path))
    listing = _run(ex.execute("list_dir", {}))
    assert listing.splitlines() == ["sub/", "a.txt"]
    assert _run(ex.execute("read_file", {"path": "missing.txt"})).startswith("Error:")


def test_executor_commands_disabled_by_default(tmp_path):
    ex = LocalExecutor(str(tmp_path))
    out = _run(ex.execute("run_command", {"command": "echo hi"}))
    assert out.startswith("Error:")
    assert "disabled" in out


def test_executor_runs_command_when_enabled(tmp_path):
    ex = LocalExecutor(str(tmp_path), allow_commands=True)
    out = _run(ex.execute("run_command", {"command": "echo hi"}))
    assert "[exit code 0]" in out
    assert "hi" in out


def test_executor_rejects_missing_root(tmp_path):
    from coder.executor import ToolExecutionError

    with pytest.raises(ToolExecutionError):
        LocalExecutor(str(tmp_path / "does-not-exist"))


# ---------------------------------------------------------------------------
# API tests with a scripted fake LLM
# ---------------------------------------------------------------------------


def _fake_llm_sequence(monkeypatch, responses: list[dict]):
    """Patch httpx.AsyncClient in coder.service to return scripted assistant messages."""
    remaining = list(responses)
    captured_payloads: list[dict] = []

    class FakeResp:
        status_code = 200

        def __init__(self, message: dict, usage: dict | None = None):
            self._body: dict = {"choices": [{"message": message}]}
            if usage is not None:
                self._body["usage"] = usage

        def json(self):
            return self._body

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            return None

        async def post(self, url, headers=None, json=None):
            assert "/chat/completions" in url
            captured_payloads.append(json)
            item = remaining.pop(0)
            if isinstance(item, tuple):
                msg, usage = item
                return FakeResp(msg, usage)
            return FakeResp(item)

    monkeypatch.setattr("coder.service.httpx.AsyncClient", lambda *a, **k: FakeClient())
    return captured_payloads


def _tool_call_msg(name: str, arguments: dict, call_id: str = "call_1") -> dict:
    return {
        "content": "",
        "tool_calls": [
            {
                "id": call_id,
                "type": "function",
                "function": {"name": name, "arguments": json.dumps(arguments)},
            }
        ],
    }


def _parse_sse(text: str) -> list[tuple[str, dict]]:
    events: list[tuple[str, dict]] = []
    for block in text.strip().split("\n\n"):
        event = ""
        data = ""
        for line in block.splitlines():
            if line.startswith("event:"):
                event = line[len("event:") :].strip()
            elif line.startswith("data:"):
                data = line[len("data:") :].strip()
        if event:
            events.append((event, json.loads(data) if data else {}))
    return events


def test_coder_stream_executes_tools_and_persists(client, monkeypatch, test_engine, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    payloads = _fake_llm_sequence(
        monkeypatch,
        [
            _tool_call_msg("write_file", {"path": "hello.txt", "content": "hello world"}),
            {"content": "Created hello.txt with a greeting."},
        ],
    )

    created = c.post(
        "/api/v1/coder/chat/threads",
        json={"workspace_root": str(tmp_path)},
        headers=auth,
    )
    assert created.status_code == 200
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/coder/chat/stream",
        json={"message": "Create hello.txt", "thread_id": thread_id, "model": "test-model"},
        headers=auth,
    )
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("text/event-stream")

    events = _parse_sse(r.text)
    kinds = [e[0] for e in events]
    assert kinds == ["tool_call", "tool_result", "assistant", "done"]
    assert events[0][1] == {
        "name": "write_file",
        "arguments": {"path": "hello.txt", "content": "hello world"},
    }
    assert "hello.txt" in events[1][1]["content"]
    assert events[2][1]["content"] == "Created hello.txt with a greeting."

    # The tool actually ran against the workspace on disk.
    assert (tmp_path / "hello.txt").read_text(encoding="utf-8") == "hello world"

    # Tools were sent to the LLM on every step.
    assert all(p.get("tools") for p in payloads)
    # Second step saw the tool result in the conversation.
    roles = [m["role"] for m in payloads[1]["messages"]]
    assert "tool" in roles

    # Full trace persisted: user, assistant(tool_calls), tool, assistant(final).
    done = events[3][1]
    assert done["thread_id"] == thread_id
    assert [m["role"] for m in done["messages"]] == ["user", "assistant", "tool", "assistant"]
    assert done["messages"][3]["content"] == "Created hello.txt with a greeting."

    with Session(test_engine) as session:
        rows = session.exec(select(CoderChatThread)).all()
        assert len(rows) == 1
        assert rows[0].workspace_root == str(tmp_path.resolve())


def test_coder_send_multi_step(client, monkeypatch, test_engine, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}
    (tmp_path / "app.py").write_text("print('v1')\n", encoding="utf-8")

    _fake_llm_sequence(
        monkeypatch,
        [
            _tool_call_msg("read_file", {"path": "app.py"}, call_id="call_r"),
            _tool_call_msg("write_file", {"path": "app.py", "content": "print('v2')\n"}, call_id="call_w"),
            {"content": "Bumped to v2."},
        ],
    )

    r = c.post(
        "/api/v1/coder/chat/send",
        json={"message": "Bump version", "workspace_root": str(tmp_path)},
        headers=auth,
    )
    assert r.status_code == 200
    body = r.json()
    assert body["messages"][-1]["content"] == "Bumped to v2."
    assert (tmp_path / "app.py").read_text(encoding="utf-8") == "print('v2')\n"
    # user + (assistant tool_call, tool) * 2 + final assistant
    assert [m["role"] for m in body["messages"]] == [
        "user", "assistant", "tool", "assistant", "tool", "assistant",
    ]


def test_coder_stream_pauses_for_command_approval(client, monkeypatch, test_engine, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    _fake_llm_sequence(
        monkeypatch,
        [_tool_call_msg("run_command", {"command": "echo hi"}, call_id="call_cmd")],
    )

    created = c.post(
        "/api/v1/coder/chat/threads", json={"workspace_root": str(tmp_path)}, headers=auth
    )
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/coder/chat/stream",
        json={"message": "run echo", "thread_id": thread_id, "allow_commands": True},
        headers=auth,
    )
    events = _parse_sse(r.text)
    kinds = [e[0] for e in events]
    assert kinds == ["approval_required", "done"]
    assert events[0][1] == {
        "name": "run_command",
        "call_id": "call_cmd",
        "arguments": {"command": "echo hi"},
    }
    done = events[1][1]
    assert done["pending_call"]["call_id"] == "call_cmd"
    # No tool result yet: the assistant tool_calls message is the last persisted entry.
    assert [m["role"] for m in done["messages"]] == ["user", "assistant"]
    assert done["messages"][1]["tool_calls"][0]["id"] == "call_cmd"

    with Session(test_engine) as session:
        row = session.exec(select(CoderChatThread)).first()
        assert row.get_pending_call()["call_id"] == "call_cmd"

    # A new message is blocked while a command awaits approval.
    blocked = c.post(
        "/api/v1/coder/chat/send",
        json={"message": "something else", "thread_id": thread_id},
        headers=auth,
    )
    assert blocked.status_code == 409


def test_coder_approve_executes_and_resumes(client, monkeypatch, test_engine, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    _fake_llm_sequence(
        monkeypatch,
        [_tool_call_msg("run_command", {"command": "echo hi"}, call_id="call_cmd")],
    )
    created = c.post(
        "/api/v1/coder/chat/threads", json={"workspace_root": str(tmp_path)}, headers=auth
    )
    thread_id = created.json()["thread_id"]
    c.post(
        "/api/v1/coder/chat/stream",
        json={"message": "run echo", "thread_id": thread_id, "allow_commands": True},
        headers=auth,
    )

    # Resume: model sees the command output and finishes.
    _fake_llm_sequence(monkeypatch, [{"content": "Command ran fine."}])
    r = c.post(
        "/api/v1/coder/chat/approve",
        json={"thread_id": thread_id, "call_id": "call_cmd", "approve": True},
        headers=auth,
    )
    assert r.status_code == 200
    events = _parse_sse(r.text)
    kinds = [e[0] for e in events]
    assert kinds == ["tool_call", "tool_result", "assistant", "done"]
    assert "hi" in events[1][1]["content"]
    assert events[2][1]["content"] == "Command ran fine."

    done = events[3][1]
    assert done["pending_call"] is None
    assert [m["role"] for m in done["messages"]] == ["user", "assistant", "tool", "assistant"]

    with Session(test_engine) as session:
        row = session.exec(select(CoderChatThread)).first()
        assert row.get_pending_call() is None


def test_coder_reject_command(client, monkeypatch, test_engine, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    _fake_llm_sequence(
        monkeypatch,
        [_tool_call_msg("run_command", {"command": "rm -rf /"}, call_id="call_cmd")],
    )
    created = c.post(
        "/api/v1/coder/chat/threads", json={"workspace_root": str(tmp_path)}, headers=auth
    )
    thread_id = created.json()["thread_id"]
    c.post(
        "/api/v1/coder/chat/stream",
        json={"message": "clean up", "thread_id": thread_id, "allow_commands": True},
        headers=auth,
    )

    _fake_llm_sequence(monkeypatch, [{"content": "Understood, I will not run that."}])
    r = c.post(
        "/api/v1/coder/chat/approve",
        json={"thread_id": thread_id, "call_id": "call_cmd", "approve": False},
        headers=auth,
    )
    events = _parse_sse(r.text)
    assert [e[0] for e in events] == ["tool_result", "assistant", "done"]
    assert "rejected" in events[0][1]["content"]

    with Session(test_engine) as session:
        row = session.exec(select(CoderChatThread)).first()
        assert row.get_pending_call() is None


def test_coder_approve_call_id_mismatch(client, monkeypatch, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    _fake_llm_sequence(
        monkeypatch,
        [_tool_call_msg("run_command", {"command": "echo hi"}, call_id="call_cmd")],
    )
    created = c.post(
        "/api/v1/coder/chat/threads", json={"workspace_root": str(tmp_path)}, headers=auth
    )
    thread_id = created.json()["thread_id"]
    c.post(
        "/api/v1/coder/chat/stream",
        json={"message": "run echo", "thread_id": thread_id, "allow_commands": True},
        headers=auth,
    )

    r = c.post(
        "/api/v1/coder/chat/approve",
        json={"thread_id": thread_id, "call_id": "wrong_id", "approve": True},
        headers=auth,
    )
    events = _parse_sse(r.text)
    assert events[0][0] == "error"
    assert "mismatch" in events[0][1]["detail"]


def test_coder_auto_approve_commands_runs_immediately(client, monkeypatch, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    _fake_llm_sequence(
        monkeypatch,
        [
            _tool_call_msg("run_command", {"command": "echo hi"}, call_id="call_cmd"),
            {"content": "Done."},
        ],
    )
    created = c.post(
        "/api/v1/coder/chat/threads", json={"workspace_root": str(tmp_path)}, headers=auth
    )
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/coder/chat/stream",
        json={
            "message": "run echo",
            "thread_id": thread_id,
            "allow_commands": True,
            "auto_approve_commands": True,
        },
        headers=auth,
    )
    events = _parse_sse(r.text)
    assert [e[0] for e in events] == ["tool_call", "tool_result", "assistant", "done"]
    assert events[3][1]["pending_call"] is None


def test_coder_stream_errors_without_workspace(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    monkeypatch.delenv("CODER_WORKSPACE_ROOT", raising=False)
    auth = {"Authorization": "Bearer test-key"}

    created = c.post("/api/v1/coder/chat/threads", json={}, headers=auth)
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/coder/chat/stream",
        json={"message": "hi", "thread_id": thread_id},
        headers=auth,
    )
    assert r.status_code == 200
    events = _parse_sse(r.text)
    assert events[0][0] == "error"
    assert "workspace_root" in events[0][1]["detail"]


def test_truncate_history_for_retry():
    from coder.service import _truncate_history_for_retry

    history = [
        {"role": "user", "content": "Plan the change"},
        {"role": "assistant", "content": "partial"},
        {"role": "tool", "tool_call_id": "c1", "name": "read_file", "content": "x"},
    ]
    assert _truncate_history_for_retry(history) == [
        {"role": "user", "content": "Plan the change"},
    ]


def test_coder_stream_retry_truncates_tail(client, monkeypatch, test_engine, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    created = c.post(
        "/api/v1/coder/chat/threads",
        json={"workspace_root": str(tmp_path)},
        headers=auth,
    )
    thread_id = created.json()["thread_id"]

    with Session(test_engine) as session:
        thread = session.get(CoderChatThread, thread_id)
        thread.set_messages(
            [
                {"role": "user", "content": "Create hello.txt"},
                {"role": "assistant", "content": "partial failure"},
            ]
        )
        session.add(thread)
        session.commit()

    _fake_llm_sequence(monkeypatch, [{"content": "Retried answer."}])

    r = c.post(
        "/api/v1/coder/chat/retry",
        json={"thread_id": thread_id, "workspace_root": str(tmp_path)},
        headers=auth,
    )
    assert r.status_code == 200
    events = _parse_sse(r.text)
    assert [e[0] for e in events] == ["assistant", "done"]
    messages = events[-1][1]["messages"]
    assert len(messages) == 2
    assert messages[0] == {"role": "user", "content": "Create hello.txt"}
    assert messages[1]["content"] == "Retried answer."


def test_coder_send_requires_master_key(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)

    r = c.post("/api/v1/coder/chat/send", json={"message": "hi"})
    assert r.status_code == 503


def test_coder_thread_crud(client, monkeypatch, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    created = c.post(
        "/api/v1/coder/chat/threads",
        json={"title": "My session", "workspace_root": str(tmp_path)},
        headers=auth,
    )
    assert created.status_code == 200
    thread_id = created.json()["thread_id"]
    assert created.json()["workspace_root"] == str(tmp_path)

    listed = c.get("/api/v1/coder/chat/threads", headers=auth)
    assert any(t["id"] == thread_id for t in listed.json()["threads"])

    fetched = c.get(f"/api/v1/coder/chat/thread?thread_id={thread_id}", headers=auth)
    assert fetched.status_code == 200
    assert fetched.json()["workspace_root"] == str(tmp_path)

    deleted = c.delete(f"/api/v1/coder/chat/thread/{thread_id}", headers=auth)
    assert deleted.json() == {"thread_id": thread_id, "deleted": True}
    assert c.get(f"/api/v1/coder/chat/thread?thread_id={thread_id}", headers=auth).status_code == 404


def test_coder_send_reports_usage_and_context(client, monkeypatch, test_engine, tmp_path):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}
    usage1 = {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110}
    usage2 = {"prompt_tokens": 120, "completion_tokens": 20, "total_tokens": 140}
    _fake_llm_sequence(
        monkeypatch,
        [
            (_tool_call_msg("write_file", {"path": "a.txt", "content": "x"}), usage1),
            ({"content": "Done."}, usage2),
        ],
    )

    created = c.post(
        "/api/v1/coder/chat/threads",
        json={"workspace_root": str(tmp_path)},
        headers=auth,
    )
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/coder/chat/send",
        json={"message": "write a.txt", "thread_id": thread_id},
        headers=auth,
    )
    assert r.status_code == 200
    body = r.json()
    assert body["usage"]["total_tokens"] == 250
    assert body["usage"]["prompt_tokens"] == 220
    assert body["usage"]["completion_tokens"] == 30
    assert len(body["usage"]["steps"]) == 2
    assert body["context_usage"]["categories"]["system_prompt"] > 0
    assert body["context_usage"]["categories"]["tools"] > 0
    assert body["context_usage"]["total_estimated"] > 0

    ctx = c.get(f"/api/v1/coder/chat/context-usage?thread_id={thread_id}", headers=auth)
    assert ctx.status_code == 200
    assert ctx.json()["categories"]["conversation"] > 0


def test_coder_stream_delegate_tools_skips_server_workspace_check(
    client, monkeypatch, test_engine
):
    """Portal Desktop paths are validated on the client, not the platform host."""
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}
    windows_root = r"D:\devstrail\devstrail"

    _fake_llm_sequence(monkeypatch, [{"content": "Hello!"}])

    created = c.post(
        "/api/v1/coder/chat/threads",
        json={"workspace_root": windows_root},
        headers=auth,
    )
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/coder/chat/stream",
        json={
            "message": "hi",
            "thread_id": thread_id,
            "workspace_root": windows_root,
            "delegate_tools": True,
        },
        headers=auth,
    )
    assert r.status_code == 200
    events = _parse_sse(r.text)
    assert events[0][0] != "error"
    assert [e[0] for e in events] == ["assistant", "done"]


def test_coder_stream_portal_desktop_header_skips_server_workspace_check(
    client, monkeypatch, test_engine
):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {
        "Authorization": "Bearer test-key",
        "X-Agent-Platform-Client": "portal-desktop",
    }
    windows_root = r"D:\devstrail\devstrail"

    _fake_llm_sequence(monkeypatch, [{"content": "Hello!"}])

    created = c.post(
        "/api/v1/coder/chat/threads",
        json={"workspace_root": windows_root},
        headers=auth,
    )
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/coder/chat/stream",
        json={"message": "hi", "thread_id": thread_id, "workspace_root": windows_root},
        headers=auth,
    )
    assert r.status_code == 200
    events = _parse_sse(r.text)
    assert events[0][0] != "error"
    assert [e[0] for e in events] == ["assistant", "done"]
