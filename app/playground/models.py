"""SQLModel table for the standalone chat Playground (no project scoping)."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Optional

from sqlmodel import Field, SQLModel

from time_utils import utc_now_naive


class PlaygroundChatThread(SQLModel, table=True):
    """A project-less chat thread; global to the platform instance."""

    __tablename__ = "playground_chat_threads"

    id: Optional[int] = Field(default=None, primary_key=True)
    title: Optional[str] = Field(default=None, max_length=128)
    messages_json: Optional[str] = Field(default=None)
    model: Optional[str] = Field(default=None, max_length=128)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_messages(self) -> list[dict[str, str]]:
        if not self.messages_json:
            return []
        try:
            data = json.loads(self.messages_json)
            return list(data) if isinstance(data, list) else []
        except json.JSONDecodeError:
            return []

    def set_messages(self, messages: list[dict[str, str]]) -> None:
        self.messages_json = json.dumps(messages, ensure_ascii=False) if messages else None
