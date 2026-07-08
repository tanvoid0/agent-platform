from __future__ import annotations

import os
from pathlib import Path
from urllib.parse import urlparse

import yaml

from llm_proxy.core.config_cache import env_file_path, resolved_config_yaml_path
from platform_config import resolved_agent_platform_yaml_path
from workspace_service import workspace_root

_VALID_ENVS = {"development", "test", "testing", "production"}


def _is_positive_float(raw: str) -> bool:
    try:
        return float(raw) > 0
    except ValueError:
        return False


def _validate_http_url(name: str, value: str, issues: list[str]) -> None:
    parsed = urlparse(value)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        issues.append(f"{name} must be a valid http(s) URL (got {value!r}).")


def _validate_yaml_file(path: Path, name: str, issues: list[str]) -> None:
    if not path.is_file():
        return
    try:
        raw = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as exc:
        issues.append(f"{name} could not be parsed: {exc}.")
        return
    if raw is not None and not isinstance(raw, dict):
        issues.append(f"{name} must contain a YAML mapping at the top level.")


def collect_startup_validation_errors() -> list[str]:
    issues: list[str] = []

    env_name = (os.getenv("AGENT_PLATFORM_ENV") or "development").strip().lower()
    if env_name not in _VALID_ENVS:
        issues.append(
            "AGENT_PLATFORM_ENV must be one of development, test, testing, or production."
        )

    master_key = (os.getenv("AGENT_PLATFORM_MASTER_KEY") or "").strip()
    if env_name == "production" and not master_key:
        issues.append("AGENT_PLATFORM_MASTER_KEY is required when AGENT_PLATFORM_ENV=production.")

    llm_base = (os.getenv("LLM_ORCHESTRATOR_BASE_URL") or "http://127.0.0.1:18410/v1").strip()
    _validate_http_url("LLM_ORCHESTRATOR_BASE_URL", llm_base, issues)

    for name in (
        "AGENT_PLATFORM_SQLITE_BUSY_TIMEOUT_SECONDS",
        "AGENT_PLATFORM_SQLITE_STARTUP_LOCK_TIMEOUT_SECONDS",
        "AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS",
    ):
        raw = (os.getenv(name) or "").strip()
        if raw and not _is_positive_float(raw):
            issues.append(f"{name} must be a positive number (got {raw!r}).")

    config_dir_raw = (os.getenv("CONFIG_DIR") or "/data").strip()
    config_dir = Path(config_dir_raw)
    if config_dir.exists() and not config_dir.is_dir():
        issues.append(f"CONFIG_DIR must point to a directory (got {config_dir_raw!r}).")

    workspace_root_raw = (os.getenv("AGENT_PLATFORM_WORKSPACE_ROOT") or "").strip()
    if workspace_root_raw:
        candidate = Path(workspace_root_raw).expanduser()
        if candidate.exists() and not candidate.is_dir():
            issues.append(
                f"AGENT_PLATFORM_WORKSPACE_ROOT must point to a directory (got {workspace_root_raw!r})."
            )

    _validate_yaml_file(resolved_agent_platform_yaml_path(), "agent_platform.yaml", issues)
    _validate_yaml_file(resolved_config_yaml_path(), "config.yaml", issues)

    try:
        workspace_root()
    except OSError as exc:
        issues.append(f"Workspace root is not accessible: {exc}.")

    env_path = env_file_path()
    if env_path.exists() and not env_path.is_file():
        issues.append(f"CONFIG_DIR/.env must be a file when present (got {str(env_path)!r}).")

    return issues


def assert_startup_config() -> None:
    issues = collect_startup_validation_errors()
    if not issues:
        return
    joined = "\n".join(f"- {issue}" for issue in issues)
    raise RuntimeError(f"Startup configuration validation failed:\n{joined}")
