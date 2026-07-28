"""SQLModel tables for model build/train operations."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any, Optional

from sqlmodel import Field, SQLModel

from time_utils import utc_now_naive


class ModelProject(SQLModel, table=True):
    __tablename__ = "model_projects"

    id: Optional[int] = Field(default=None, primary_key=True)
    name: str = Field(max_length=128, index=True, unique=True)
    description: Optional[str] = Field(default=None, max_length=512)
    manifest_json: Optional[str] = Field(default=None)
    workspace_id: Optional[int] = Field(default=None, index=True)
    created_at: datetime = Field(default_factory=utc_now_naive)
    updated_at: datetime = Field(default_factory=utc_now_naive)

    def get_manifest(self) -> dict[str, Any]:
        if not self.manifest_json:
            return {}
        try:
            data = json.loads(self.manifest_json)
            return data if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_manifest(self, manifest: dict[str, Any]) -> None:
        self.manifest_json = json.dumps(manifest, ensure_ascii=False)


class ModelBuildJob(SQLModel, table=True):
    __tablename__ = "model_build_jobs"

    id: Optional[int] = Field(default=None, primary_key=True)
    project_id: Optional[int] = Field(default=None, foreign_key="model_projects.id", index=True)
    job_type: str = Field(default="pipeline", max_length=32, index=True)
    operation_json: Optional[str] = Field(default=None)
    stages_json: str = Field(default='["prepare"]')
    status: str = Field(default="pending", max_length=32, index=True)
    current_stage: Optional[str] = Field(default=None, max_length=32)
    log_path: Optional[str] = Field(default=None, max_length=1024)
    result_json: Optional[str] = Field(default=None)
    register_alias: Optional[str] = Field(default=None, max_length=128)
    error_message: Optional[str] = Field(default=None, max_length=2048)
    process_id: Optional[int] = Field(default=None, index=True)
    created_at: datetime = Field(default_factory=utc_now_naive)
    started_at: Optional[datetime] = Field(default=None)
    finished_at: Optional[datetime] = Field(default=None)

    def get_stages(self) -> list[str]:
        try:
            data = json.loads(self.stages_json)
            return [str(s) for s in data] if isinstance(data, list) else ["prepare"]
        except json.JSONDecodeError:
            return ["prepare"]

    def set_stages(self, stages: list[str]) -> None:
        self.stages_json = json.dumps(stages)

    def get_result(self) -> dict[str, Any]:
        if not self.result_json:
            return {}
        try:
            data = json.loads(self.result_json)
            return data if isinstance(data, dict) else {}
        except json.JSONDecodeError:
            return {}

    def set_result(self, result: dict[str, Any]) -> None:
        self.result_json = json.dumps(result, ensure_ascii=False)


class ModelRegistryEntry(SQLModel, table=True):
    __tablename__ = "model_registry_entries"

    id: Optional[int] = Field(default=None, primary_key=True)
    project_id: int = Field(foreign_key="model_projects.id", index=True)
    version: str = Field(max_length=32)
    ollama_tag: str = Field(max_length=128, index=True)
    base_model: Optional[str] = Field(default=None, max_length=256)
    adapter_path: Optional[str] = Field(default=None, max_length=512)
    gguf_path: Optional[str] = Field(default=None, max_length=512)
    eval_score: Optional[float] = Field(default=None)
    is_active: bool = Field(default=False, index=True)
    metadata_json: Optional[str] = Field(default=None)
    created_at: datetime = Field(default_factory=utc_now_naive)
