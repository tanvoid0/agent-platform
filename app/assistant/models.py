"""SQLModel tables for the Personal Assistant product."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any, Optional

from sqlmodel import Field, SQLModel

from time_utils import utc_now_naive


class AssistantChatThread(SQLModel, table=True):
    """A chat session for the Personal Assistant (many per project)."""

    __tablename__ = "assistant_chat_threads"

    id: Optional[int] = Field(default=None, primary_key=True)
    project_id: int = Field(foreign_key="project.id", index=True)
    title: Optional[str] = Field(default=None, max_length=128)
    messages_json: Optional[str] = Field(default=None)
    pending_actions_json: Optional[str] = Field(default=None)
    last_profile_slug: Optional[str] = Field(default=None, max_length=64)
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

    def get_pending_actions(self) -> list[dict[str, Any]]:
        if not self.pending_actions_json:
            return []
        try:
            data = json.loads(self.pending_actions_json)
            return list(data) if isinstance(data, list) else []
        except json.JSONDecodeError:
            return []

    def set_pending_actions(self, actions: list[dict[str, Any]]) -> None:
        self.pending_actions_json = json.dumps(actions, ensure_ascii=False) if actions else None


class AssistantDomainProfile(SQLModel, table=True):
    """Persistent user/domain context per project (fitness stats, travel prefs, etc.)."""

    __tablename__ = "assistant_domain_profiles"

    id: Optional[int] = Field(default=None, primary_key=True)
    project_id: int = Field(foreign_key="project.id", index=True)
    domain: str = Field(max_length=64, index=True)
    profile_json: Optional[str] = Field(default=None)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_profile(self) -> dict[str, Any]:
        if not self.profile_json:
            return {}
        try:
            data = json.loads(self.profile_json)
            return dict(data) if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_profile(self, data: dict[str, Any]) -> None:
        self.profile_json = json.dumps(data, ensure_ascii=False) if data else None


class AssistantReview(SQLModel, table=True):
    """Reviewer check-in session with proposed plan adjustments."""

    __tablename__ = "assistant_reviews"

    id: Optional[int] = Field(default=None, primary_key=True)
    project_id: int = Field(foreign_key="project.id", index=True)
    status: str = Field(default="pending", max_length=32)
    summary: Optional[str] = Field(default=None)
    stats_json: Optional[str] = Field(default=None)
    proposed_actions_json: Optional[str] = Field(default=None)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_stats(self) -> dict[str, Any]:
        if not self.stats_json:
            return {}
        try:
            data = json.loads(self.stats_json)
            return dict(data) if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_stats(self, data: dict[str, Any]) -> None:
        self.stats_json = json.dumps(data, ensure_ascii=False) if data else None

    def get_proposed_actions(self) -> list[dict[str, Any]]:
        if not self.proposed_actions_json:
            return []
        try:
            data = json.loads(self.proposed_actions_json)
            return list(data) if isinstance(data, list) else []
        except json.JSONDecodeError:
            return []

    def set_proposed_actions(self, actions: list[dict[str, Any]]) -> None:
        self.proposed_actions_json = (
            json.dumps(actions, ensure_ascii=False) if actions else None
        )
