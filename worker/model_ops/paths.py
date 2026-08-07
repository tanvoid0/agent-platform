"""Resolve MODEL_OPS_DATA_DIR and ensure scaffold files exist."""

from __future__ import annotations

import os
import shutil
from pathlib import Path


_PKG_ROOT = Path(__file__).resolve().parent
_BUNDLED_DATA = _PKG_ROOT / "data"


def _resolved_config_dir() -> Path:
    """`platform_config.resolved_config_dir`, inlined.

    The worker no longer imports from the API server package, so the one
    function it needed from it lives here. ``agent-platformd`` always passes
    ``CONFIG_DIR`` and ``MODEL_OPS_DATA_DIR`` through to the stage subprocess,
    so the fallback below is only reached when a stage is run by hand.
    """
    explicit = (os.environ.get("CONFIG_DIR") or "").strip()
    if explicit:
        return Path(explicit)
    return Path.cwd() / "data" / "llm"


def data_dir() -> Path:
    raw = os.environ.get("MODEL_OPS_DATA_DIR", "").strip()
    if raw:
        return Path(raw)
    return _resolved_config_dir() / "model_ops"


def projects_dir() -> Path:
    return data_dir() / "projects"


def defaults_path() -> Path:
    return data_dir() / "defaults.yaml"


def template_project_dir() -> Path:
    return projects_dir() / "_template"


def ensure_data_scaffold() -> Path:
    """Create data dir, defaults.yaml, and _template project if missing."""
    root = data_dir()
    root.mkdir(parents=True, exist_ok=True)
    projects_dir().mkdir(parents=True, exist_ok=True)

    dest_defaults = defaults_path()
    if not dest_defaults.exists() and (_BUNDLED_DATA / "defaults.yaml").exists():
        shutil.copy2(_BUNDLED_DATA / "defaults.yaml", dest_defaults)

    dest_template = template_project_dir()
    bundled_template = _BUNDLED_DATA / "projects" / "_template"
    if bundled_template.is_dir() and not dest_template.exists():
        shutil.copytree(bundled_template, dest_template)

    return root
