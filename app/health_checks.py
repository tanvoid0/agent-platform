from __future__ import annotations

from typing import Any

import database
from llm_proxy.core.provider_config import first_configured_provider
from workspace_service import workspace_root


def _check_database() -> tuple[bool, str]:
    try:
        with database.engine.connect() as conn:
            conn.exec_driver_sql("SELECT 1")
        return True, "database reachable"
    except Exception as exc:
        return False, f"database check failed: {exc}"


def _check_workspace_root() -> tuple[bool, str]:
    try:
        root = workspace_root()
        return True, f"workspace root ready at {root}"
    except Exception as exc:
        return False, f"workspace root unavailable: {exc}"


def app_readiness_payload() -> tuple[int, dict[str, Any]]:
    checks: list[dict[str, Any]] = []
    status_code = 200
    for name, fn in (
        ("database", _check_database),
        ("workspace_root", _check_workspace_root),
    ):
        ok, detail = fn()
        checks.append({"name": name, "ok": ok, "detail": detail})
        if not ok:
            status_code = 503
    return status_code, {"status": "ok" if status_code == 200 else "unready", "checks": checks}


def llm_proxy_readiness_payload() -> tuple[int, dict[str, Any]]:
    checks: list[dict[str, Any]] = []
    provider = first_configured_provider()
    if provider:
        checks.append(
            {
                "name": "provider_config",
                "ok": True,
                "detail": f"default provider can resolve to {provider}",
            }
        )
        return 200, {"status": "ok", "checks": checks}

    checks.append(
        {
            "name": "provider_config",
            "ok": False,
            "detail": "No supported LLM provider is configured. Set provider credentials or local backend bases.",
        }
    )
    return 503, {"status": "unready", "checks": checks}
