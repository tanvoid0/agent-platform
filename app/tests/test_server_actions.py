"""Tests for server-side todo actions."""

import pytest

from todos.services.server_actions import execute_trigger_webhook


def test_execute_trigger_webhook_rejects_bad_url():
    with pytest.raises(ValueError, match="http"):
        execute_trigger_webhook("ftp://bad.example/hook")


def test_execute_trigger_webhook_posts(monkeypatch):
    class FakeResponse:
        status_code = 204
        text = ""

        @property
        def is_success(self) -> bool:
            return True

    class FakeClient:
        def __init__(self, timeout: float):
            self.timeout = timeout

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

        def post(self, url: str, json: dict):
            assert url == "https://hooks.example/run"
            assert json == {"task": "done"}
            return FakeResponse()

    monkeypatch.setattr("todos.services.server_actions.httpx.Client", FakeClient)
    result = execute_trigger_webhook("https://hooks.example/run", {"task": "done"})
    assert result["ok"] is True
    assert result["status_code"] == 204
