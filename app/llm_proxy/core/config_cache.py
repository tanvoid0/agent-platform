"""Cached reads for config files and .env with mtime/size invalidation.

Avoids re-parsing YAML and re-reading .env on every request while staying
consistent when files are edited on disk (including by this process).
"""

from __future__ import annotations

import os
import threading
from pathlib import Path
from typing import Any

import yaml

_file_lock = threading.Lock()


def resolved_config_yaml_path() -> Path:
    """Same resolution as the OpenAI proxy routes (supports CONFIG_PATH)."""
    root = Path(os.environ.get("CONFIG_DIR", "/data"))
    explicit = os.environ.get("CONFIG_PATH", "").strip()
    if explicit:
        return Path(explicit)
    return root / "config.yaml"


def env_file_path() -> Path:
    return Path(os.environ.get("CONFIG_DIR", "/data")) / ".env"


def llm_proxy_ui_yaml_path() -> Path:
    """Legacy filename on disk: ``orchestrator_ui.yaml`` (fallback_models for the proxy UI)."""
    return Path(os.environ.get("CONFIG_DIR", "/data")) / "orchestrator_ui.yaml"


def _stat_fp(path: Path) -> tuple[int, int] | None:
    try:
        st = path.stat()
        return (st.st_mtime_ns, st.st_size)
    except OSError:
        return None


def _parse_env_file(content: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for line in content.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        k, _, v = line.partition("=")
        k = k.strip()
        v = v.strip()
        if len(v) >= 2 and v[0] == v[-1] and v[0] in "\"'":
            v = v[1:-1]
        out[k] = v
    return out


_yaml_fp: tuple[int, int] | None = None
_yaml_data: dict[str, Any] = {}


def load_config_yaml_dict() -> dict[str, Any]:
    """Parsed config.yaml; refreshed when the file changes (mtime + size)."""
    global _yaml_fp, _yaml_data
    path = resolved_config_yaml_path()
    with _file_lock:
        fp = _stat_fp(path)
        if fp is not None and fp == _yaml_fp:
            return _yaml_data
        if fp is None or not path.is_file():
            _yaml_fp = fp
            _yaml_data = {}
            return {}
        try:
            raw = yaml.safe_load(path.read_text(encoding="utf-8"))
            data = raw if isinstance(raw, dict) else {}
        except (yaml.YAMLError, OSError):
            data = {}
        fp2 = _stat_fp(path)
        _yaml_fp = fp2
        _yaml_data = data
        return data


_env_fp: tuple[int, int] | None = None
_env_data: dict[str, str] = {}


def read_env_file_parsed() -> dict[str, str]:
    """Parsed .env from CONFIG_DIR; refreshed when the file changes."""
    global _env_fp, _env_data
    path = env_file_path()
    with _file_lock:
        fp = _stat_fp(path)
        if fp is not None and fp == _env_fp:
            return _env_data
        if fp is None or not path.is_file():
            _env_fp = fp
            _env_data = {}
            return {}
        try:
            data = _parse_env_file(path.read_text(encoding="utf-8"))
        except OSError:
            data = {}
        fp2 = _stat_fp(path)
        _env_fp = fp2
        _env_data = data
        return data


_llm_proxy_ui_fp: tuple[int, int] | None = None
_llm_proxy_ui_fb: dict[str, list[str]] = {}


def read_llm_proxy_ui_fallbacks() -> dict[str, list[str]]:
    """``orchestrator_ui.yaml`` fallback_models map; refreshed when the file changes."""
    global _llm_proxy_ui_fp, _llm_proxy_ui_fb
    path = llm_proxy_ui_yaml_path()
    with _file_lock:
        fp = _stat_fp(path)
        if fp is not None and fp == _llm_proxy_ui_fp:
            return _llm_proxy_ui_fb
        if fp is None or not path.is_file():
            _llm_proxy_ui_fp = fp
            _llm_proxy_ui_fb = {}
            return {}
        try:
            raw = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
        except (yaml.YAMLError, OSError):
            raw = {}
        fb = raw.get("fallback_models") if isinstance(raw, dict) else None
        out: dict[str, list[str]] = {}
        if isinstance(fb, dict):
            for k, v in fb.items():
                if not isinstance(k, str) or not k.strip():
                    continue
                if isinstance(v, list):
                    out[k.strip()] = [str(x).strip() for x in v if str(x).strip()]
                elif isinstance(v, str) and v.strip():
                    out[k.strip()] = [v.strip()]
        fp2 = _stat_fp(path)
        _llm_proxy_ui_fp = fp2
        _llm_proxy_ui_fb = out
        return out
