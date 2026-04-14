"""
Optional `config/agent_platform.yaml` for non-secret defaults.

Precedence: process env > `.env` (via python-dotenv) > YAML `env` map > code defaults.

Secrets (`AGENT_PLATFORM_MASTER_KEY`, `GEMINI_API_KEY`,
`LM_STUDIO_API_KEY`) are never read from YAML — set them only in `.env` or the environment.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import yaml

# Never apply these from YAML (must come from real env or agent-platform/.env).
_SECRET_ENV_KEYS: frozenset[str] = frozenset(
    {
        "AGENT_PLATFORM_MASTER_KEY",
        "GEMINI_API_KEY",
        "LM_STUDIO_API_KEY",
    }
)


def resolved_agent_platform_yaml_path() -> Path:
    """`AGENT_PLATFORM_CONFIG_YAML` or `agent-platform/config/agent_platform.yaml`."""
    explicit = (os.environ.get("AGENT_PLATFORM_CONFIG_YAML") or "").strip()
    if explicit:
        return Path(explicit)
    here = Path(__file__).resolve().parent
    return here.parent / "config" / "agent_platform.yaml"


def _stringify_env_value(v: Any) -> str:
    if isinstance(v, bool):
        return "1" if v else "0"
    if v is None:
        return ""
    if isinstance(v, (int, float)):
        return str(v)
    return str(v).strip()


def _env_map_from_yaml(raw: dict[str, Any]) -> dict[str, str]:
    block = raw.get("env")
    if not isinstance(block, dict):
        return {}
    out: dict[str, str] = {}
    for k, v in block.items():
        if not isinstance(k, str) or not k.strip():
            continue
        key = k.strip()
        if key in _SECRET_ENV_KEYS:
            continue
        out[key] = _stringify_env_value(v)
    return out


def apply_platform_yaml_defaults() -> None:
    """
    After `load_dotenv`: merge YAML into `os.environ` with `setdefault` only.

    Idempotent for a given process: second call re-reads file if implementation changes;
    callers should invoke once at startup.
    """
    path = resolved_agent_platform_yaml_path()
    if not path.is_file():
        return
    try:
        raw = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (yaml.YAMLError, OSError):
        return
    if not isinstance(raw, dict):
        return
    for k, val in _env_map_from_yaml(raw).items():
        if k:
            os.environ.setdefault(k, val)
