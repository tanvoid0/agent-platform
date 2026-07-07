"""SQLModel table for Coder agent chat threads (workspace-bound, project-less)."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any, Optional

from sqlmodel import Field, SQLModel

from time_utils import utc_now_naive


class CoderChatThread(SQLModel, table=True):
    """A coding-agent thread pinned to one workspace directory."""

    __tablename__ = "coder_chat_threads"

    id: Optional[int] = Field(default=None, primary_key=True)
    title: Optional[str] = Field(default=None, max_length=128)
    workspace_root: Optional[str] = Field(default=None, max_length=1024)
    messages_json: Optional[str] = Field(default=None)
    model: Optional[str] = Field(default=None, max_length=128)
    pending_call_json: Optional[str] = Field(default=None)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_messages(self) -> list[dict[str, Any]]:
        if not self.messages_json:
            return []
        try:
            data = json.loads(self.messages_json)
            return list(data) if isinstance(data, list) else []
        except json.JSONDecodeError:
            return []

    def set_messages(self, messages: list[dict[str, Any]]) -> None:
        self.messages_json = json.dumps(messages, ensure_ascii=False) if messages else None

    def get_pending_call(self) -> dict[str, Any] | None:
        if not self.pending_call_json:
            return None
        try:
            data = json.loads(self.pending_call_json)
            return data if isinstance(data, dict) else None
        except json.JSONDecodeError:
            return None

    def set_pending_call(self, call: dict[str, Any] | None) -> None:
        self.pending_call_json = json.dumps(call, ensure_ascii=False) if call else None
