"""Workflow execution: template resolution, step execution, interval scheduler."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
from datetime import timedelta
from typing import Any

import httpx
from sqlmodel import Session, select

import database
from action_orchestrator.models import Action
from time_utils import utc_now_naive
from workflows.models import Workflow, WorkflowRun

logger = logging.getLogger(__name__)

_TEMPLATE_RE = re.compile(r"\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}")
_WHOLE_TEMPLATE_RE = re.compile(r"^\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}$")

_MAX_OUTPUT_BYTES = 65536
_DEFAULT_HTTP_TIMEOUT = 30.0
_MAX_HTTP_TIMEOUT = 120.0


class StepError(Exception):
    pass


def _resolve_path(path: str, ctx: dict[str, Any]) -> Any:
    cur: Any = ctx
    for part in path.split("."):
        if isinstance(cur, dict):
            if part not in cur:
                raise StepError(f"template path not found: {path}")
            cur = cur[part]
        elif isinstance(cur, list):
            try:
                cur = cur[int(part)]
            except (ValueError, IndexError):
                raise StepError(f"template path not found: {path}")
        else:
            raise StepError(f"template path not found: {path}")
    return cur


def resolve_templates(value: Any, ctx: dict[str, Any]) -> Any:
    """Substitute {{path}} references. A string that is exactly one template
    resolves to the referenced value (keeping its type); templates embedded in
    a longer string are stringified."""
    if isinstance(value, str):
        whole = _WHOLE_TEMPLATE_RE.fullmatch(value)
        if whole:
            return _resolve_path(whole.group(1), ctx)
        return _TEMPLATE_RE.sub(lambda m: str(_resolve_path(m.group(1), ctx)), value)
    if isinstance(value, dict):
        return {k: resolve_templates(v, ctx) for k, v in value.items()}
    if isinstance(value, list):
        return [resolve_templates(v, ctx) for v in value]
    return value


def _parse_response_body(resp: httpx.Response) -> Any:
    raw = resp.content[:_MAX_OUTPUT_BYTES]
    try:
        return json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return raw.decode("utf-8", errors="replace")


async def _execute_http(params: dict[str, Any]) -> Any:
    url = params["url"]
    if not str(url).lower().startswith(("http://", "https://")):
        raise StepError(f"unsupported url scheme: {url}")
    timeout = min(float(params.get("timeout_seconds", _DEFAULT_HTTP_TIMEOUT)), _MAX_HTTP_TIMEOUT)
    method = str(params.get("method", "GET")).upper()
    headers = params.get("headers") or {}
    body = params.get("body")
    kwargs: dict[str, Any] = {"headers": headers}
    if body is not None:
        if isinstance(body, (dict, list)):
            kwargs["json"] = body
        else:
            kwargs["content"] = str(body)
    try:
        async with httpx.AsyncClient(timeout=timeout, follow_redirects=True) as client:
            resp = await client.request(method, url, **kwargs)
    except httpx.HTTPError as e:
        raise StepError(f"http request failed: {e}")
    output = {"status": resp.status_code, "body": _parse_response_body(resp)}
    if resp.status_code >= 400:
        raise StepError(f"http {resp.status_code} from {url}: {json.dumps(output['body'])[:500]}")
    return output


async def _execute_action(params: dict[str, Any], session: Session) -> Any:
    action = session.exec(
        select(Action)
        .where(Action.set_id == int(params["action_set_id"]))
        .where(Action.action_id == str(params["action_id"]))
    ).first()
    if not action:
        raise StepError(f"action not found: set {params['action_set_id']} / {params['action_id']}")
    if action.execution_mode != "server" or not action.endpoint:
        raise StepError(
            f"action '{action.action_id}' is not server-executable "
            "(needs execution_mode 'server' and an endpoint)"
        )
    return await _execute_http(
        {"url": action.endpoint, "method": "POST", "body": params.get("arguments") or {}}
    )


async def execute_workflow(
    workflow_id: int,
    input_data: dict[str, Any] | None = None,
    trigger: str = "manual",
) -> WorkflowRun:
    """Run all steps sequentially; stop at first failure. Returns the finished run."""
    with Session(database.engine) as session:
        workflow = session.get(Workflow, workflow_id)
        if not workflow:
            raise ValueError(f"workflow {workflow_id} not found")
        steps = workflow.get_steps()

        run = WorkflowRun(workflow_id=workflow_id, trigger=trigger, status="running")
        run.set_input(input_data or {})
        session.add(run)
        session.commit()
        session.refresh(run)

        ctx: dict[str, Any] = {"trigger": {"body": input_data or {}}, "steps": {}}
        results: list[dict[str, Any]] = []
        failed: str | None = None

        for step in steps:
            step_id = step["id"]
            started = time.monotonic()
            try:
                params = resolve_templates(step.get("params") or {}, ctx)
                if step["type"] == "http":
                    output = await _execute_http(params)
                elif step["type"] == "action":
                    output = await _execute_action(params, session)
                else:
                    raise StepError(f"unknown step type: {step['type']}")
                ctx["steps"][step_id] = {"output": output}
                results.append(
                    {
                        "id": step_id,
                        "status": "succeeded",
                        "output": output,
                        "duration_ms": int((time.monotonic() - started) * 1000),
                    }
                )
            except StepError as e:
                failed = f"step '{step_id}': {e}"
                results.append(
                    {
                        "id": step_id,
                        "status": "failed",
                        "error": str(e),
                        "duration_ms": int((time.monotonic() - started) * 1000),
                    }
                )
                break

        if failed:
            remaining = {s["id"] for s in steps} - {r["id"] for r in results}
            results.extend({"id": sid, "status": "skipped"} for sid in remaining)

        run.status = "failed" if failed else "succeeded"
        run.error = failed
        run.set_step_results(results)
        run.finished_at = utc_now_naive()
        session.add(run)
        session.commit()
        session.refresh(run)
        return run


# === Interval scheduler ===

_POLL_SECONDS = 30.0
_scheduler_task: asyncio.Task | None = None


def _scheduler_enabled() -> bool:
    return (os.getenv("AGENT_PLATFORM_WORKFLOW_SCHEDULER") or "1").strip() != "0"


async def _run_due_workflows() -> None:
    now = utc_now_naive()
    with Session(database.engine) as session:
        due = session.exec(
            select(Workflow)
            .where(Workflow.enabled == True)  # noqa: E712 — SQL expression
            .where(Workflow.interval_seconds != None)  # noqa: E711
            .where(Workflow.next_run_at != None)  # noqa: E711
            .where(Workflow.next_run_at <= now)
        ).all()
        for wf in due:
            # Advance next_run_at before executing so a crash can't tight-loop one workflow.
            wf.next_run_at = now + timedelta(seconds=wf.interval_seconds)
            session.add(wf)
        session.commit()
        due_ids = [wf.id for wf in due]

    for wf_id in due_ids:
        try:
            run = await execute_workflow(wf_id, trigger="schedule")
            logger.info("Scheduled workflow %s finished: %s", wf_id, run.status)
        except Exception:
            logger.exception("Scheduled workflow %s crashed", wf_id)


async def _scheduler_loop() -> None:
    while True:
        await asyncio.sleep(_POLL_SECONDS)
        try:
            await _run_due_workflows()
        except Exception:
            logger.exception("Workflow scheduler tick failed")


def start_scheduler() -> None:
    global _scheduler_task
    if _scheduler_enabled() and _scheduler_task is None:
        _scheduler_task = asyncio.get_event_loop().create_task(_scheduler_loop())
        logger.info("Workflow scheduler started (poll every %ss)", _POLL_SECONDS)


def stop_scheduler() -> None:
    global _scheduler_task
    if _scheduler_task is not None:
        _scheduler_task.cancel()
        _scheduler_task = None
