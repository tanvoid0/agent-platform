from __future__ import annotations

import json
import re
from pathlib import Path

from shared_enums import (
    ProcessRetryMode,
    ProcessStatus,
    ProcessSyncAction,
    ReviewDecision,
    TaskStatus,
    enum_values,
)


def _read_generated_enums_ts() -> str:
    repo_root = Path(__file__).resolve().parents[2]
    return (repo_root / "web" / "src" / "api" / "enums.ts").read_text(encoding="utf-8")


def _extract_const_array(source: str, const_name: str) -> tuple[str, ...]:
    pattern = re.compile(rf"export const {const_name} = \[(.*?)\] as const;", re.DOTALL)
    match = pattern.search(source)
    assert match, f"{const_name} not found in generated enums.ts"
    payload = f"[{match.group(1)}]"
    return tuple(json.loads(payload))


def test_generated_frontend_enums_match_backend_source() -> None:
    source = _read_generated_enums_ts()
    assert _extract_const_array(source, "PROCESS_STATUSES") == enum_values(ProcessStatus)
    assert _extract_const_array(source, "TASK_STATUSES") == enum_values(TaskStatus)
    assert _extract_const_array(source, "REVIEW_DECISIONS") == enum_values(ReviewDecision)
    assert _extract_const_array(source, "PROCESS_RETRY_MODES") == enum_values(ProcessRetryMode)
    assert _extract_const_array(source, "PROCESS_SYNC_ACTIONS") == enum_values(ProcessSyncAction)

