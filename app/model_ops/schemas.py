"""Pydantic request/response models for model-ops API."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field

JobStatus = Literal["pending", "running", "succeeded", "failed", "cancelled"]
PipelineStage = Literal["prepare", "train", "export", "eval"]


class OllamaModelSummary(BaseModel):
    name: str
    size: int | None = None
    modified_at: str | None = Field(default=None, alias="modified_at")
    details: dict[str, Any] | None = None

    model_config = {"populate_by_name": True}


class OllamaModelsOut(BaseModel):
    models: list[dict[str, Any]] = Field(description="Ollama tag list from /api/tags")


class OllamaModelShowOut(BaseModel):
    name: str | None = None
    modelfile: str | None = None
    parameters: str | None = None
    details: dict[str, Any] | None = None
    raw: dict[str, Any] = Field(default_factory=dict)


class OllamaPullRequest(BaseModel):
    name: str = Field(examples=["llama3.2:latest"])
    async_job: bool = Field(
        default=False,
        alias="async",
        description="When true, enqueue a tracked job instead of blocking until pull completes.",
    )

    model_config = {"populate_by_name": True}


class OllamaCopyRequest(BaseModel):
    source: str = Field(examples=["llama3.2:latest"])
    destination: str = Field(examples=["my-app:latest"])
    async_job: bool = Field(default=True, alias="async")

    model_config = {"populate_by_name": True}


class OllamaJobCreateRequest(BaseModel):
    operation: Literal["pull", "copy"] = Field(examples=["pull"])
    name: str | None = Field(default=None, description="Model name for pull")
    source: str | None = Field(default=None, description="Source for copy")
    destination: str | None = Field(default=None, description="Destination for copy")


class OllamaCreateRequest(BaseModel):
    name: str = Field(examples=["my-custom-model"])
    modelfile: str | None = Field(
        default=None,
        description="Modelfile contents. Alternative: pass structured fields via `from_model`, `system`, etc.",
        examples=["FROM gemma4:latest\nSYSTEM You are a helpful assistant."],
    )
    from_model: str | None = Field(default=None, alias="from", examples=["gemma4:latest"])
    system: str | None = None
    quantize: str | None = None

    model_config = {"populate_by_name": True}


class OllamaOperationOut(BaseModel):
    ok: bool
    message: str
    events: list[dict[str, Any]] = Field(default_factory=list)


class ModelProjectCreateRequest(BaseModel):
    name: str = Field(min_length=1, max_length=128, pattern=r"^[a-zA-Z0-9_-]+$")
    description: str | None = None
    base_model: str | None = Field(default=None, examples=["google/gemma-3-4b-it"])
    ollama_tag: str | None = None


class ModelProjectOut(BaseModel):
    id: int
    name: str
    description: str | None = None
    manifest: dict[str, Any] = Field(default_factory=dict)
    registry_entries: list["ModelRegistryEntryOut"] = Field(default_factory=list)


class ModelProjectsListOut(BaseModel):
    projects: list[ModelProjectOut]


class ModelRegistryEntryOut(BaseModel):
    id: int
    project_id: int
    project_name: str | None = None
    version: str
    ollama_tag: str
    base_model: str | None = None
    eval_score: float | None = None
    is_active: bool = False


class ModelRegistryListOut(BaseModel):
    entries: list[ModelRegistryEntryOut]


class ModelBuildJobCreateRequest(BaseModel):
    project: str = Field(examples=["my-app"])
    stages: list[PipelineStage] = Field(
        default=["prepare", "train", "export", "eval"],
        examples=[["prepare", "export", "eval"]],
    )
    register_alias: str | None = Field(
        default=None,
        description="Optional config.yaml alias to register after successful export",
    )
    offline_eval: bool = Field(default=False, description="Run eval offline (no Ollama calls)")
    process_id: int | None = Field(default=None, description="Link job to an orchestration process")


class ModelBuildJobOut(BaseModel):
    id: int
    job_type: str = "pipeline"
    project_id: int | None = None
    project_name: str | None = None
    stages: list[str] = Field(default_factory=list)
    status: JobStatus
    current_stage: str | None = None
    register_alias: str | None = None
    result: dict[str, Any] = Field(default_factory=dict)
    error_message: str | None = None
    log_tail: str | None = None
    poll_url: str
    stream_url: str
    created_at: str
    started_at: str | None = None
    finished_at: str | None = None


class ModelBuildOperationRequest(BaseModel):
    operation: str = Field(default="model.build", examples=["model.build"])
    input: ModelBuildJobCreateRequest


class ModelBuildOperationOut(BaseModel):
    operation: str = "model.build"
    job_id: int
    poll_url: str
    stream_url: str
