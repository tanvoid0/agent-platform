"""Shared utilities for loading model-ops project manifests."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import yaml

from model_ops.paths import data_dir, defaults_path, ensure_data_scaffold, projects_dir

_defaults_cache: dict[str, Any] | None = None


def load_defaults() -> dict[str, Any]:
    global _defaults_cache
    if _defaults_cache is not None:
        return _defaults_cache

    ensure_data_scaffold()
    path = defaults_path()
    if not path.exists():
        raise FileNotFoundError(
            f"Missing {path}. Restore defaults.yaml with at least base_model and ollama_base_model."
        )

    with path.open(encoding="utf-8") as f:
        data = yaml.safe_load(f) or {}

    if not isinstance(data, dict):
        raise ValueError(f"Invalid {path}: expected a YAML mapping at the top level.")

    _defaults_cache = data
    return data


def require_base_model(manifest: dict[str, Any], project: str | None = None) -> str:
    base = manifest.get("base_model")
    if isinstance(base, str) and base.strip():
        return base.strip()

    project_name = project or manifest.get("_name") or manifest.get("name") or "<project>"
    manifest_path = projects_dir() / project_name / "project.yaml"
    defaults = load_defaults()
    example = defaults.get("base_model", "<huggingface-model-id>")

    raise ValueError(
        f"Missing required 'base_model' in {manifest_path}.\n"
        f"Add a HuggingFace model ID, e.g.:\n"
        f"  base_model: {example}\n"
        f"See {defaults_path()} for the current default."
    )


def get_ollama_base_model() -> str:
    defaults = load_defaults()
    model = defaults.get("ollama_base_model")
    if isinstance(model, str) and model.strip():
        return model.strip()

    raise ValueError(
        f"Missing required 'ollama_base_model' in {defaults_path()}.\n"
        "Add an Ollama model tag, e.g.:\n"
        "  ollama_base_model: gemma4:latest"
    )


def get_project_dir(project: str) -> Path:
    ensure_data_scaffold()
    path = projects_dir() / project
    if not path.is_dir():
        raise FileNotFoundError(f"Project not found: {project} ({path})")
    return path


def load_project(project: str) -> dict[str, Any]:
    project_dir = get_project_dir(project)
    manifest_path = project_dir / "project.yaml"
    if not manifest_path.exists():
        raise FileNotFoundError(f"Missing project.yaml: {manifest_path}")
    with manifest_path.open(encoding="utf-8") as f:
        data = yaml.safe_load(f)
    data["_project_dir"] = str(project_dir)
    data["_name"] = project
    return data


def project_path(project: str, *parts: str) -> Path:
    return get_project_dir(project).joinpath(*parts)


def load_input_schema(project: str) -> dict[str, Any]:
    manifest = load_project(project)
    schema_rel = manifest.get("input_schema", "schemas/input.schema.json")
    schema_path = project_path(project, schema_rel)
    with schema_path.open(encoding="utf-8") as f:
        return json.load(f)


def data_root() -> Path:
    return data_dir()
