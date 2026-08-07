"""AI assistant for workflows: generate, review or edit step JSON via chat.

One endpoint, Notion-style: the user says what they want, the current draft (if
any) rides along as context, and the model answers with prose plus — when it
actually changed something — a full replacement steps array. Validation runs
before anything reaches the caller, so the UI can drop the steps straight into
the editor.
"""

from __future__ import annotations

import json
import logging
from typing import Any

from pydantic import BaseModel

from llm_client import call_llm
from llm_text import strip_code_fences
from workflows.schemas import WorkflowStep, validate_steps

logger = logging.getLogger(__name__)

SYSTEM_PROMPT = """You help users build and review workflow automations.

A workflow is a JSON array of steps, run strictly top to bottom. Each step:
  {"id": "<slug>", "type": "http" | "action", "params": {...}}

- "http" params: url (required), method (GET/POST/PUT/PATCH/DELETE/HEAD,
  default GET), headers (object), body (JSON), timeout_seconds.
- "action" params: action_set_id (int), action_id (string), arguments (object).
  Actions are pre-registered server-executed endpoints.
- Step ids are slugs: lowercase letter first, then [a-z0-9_-], unique.
- Templates pass data between steps: {{trigger.body.<path>}} is the JSON the
  caller sent when triggering the run; {{steps.<id>.output.body.<path>}} and
  {{steps.<id>.output.status}} read earlier http/action responses. Lists index
  numerically: {{steps.a.output.body.items.0.name}}. A string that is exactly
  one template keeps the referenced value's type.
- A failing step (non-2xx, timeout, missing template path) stops the run.

Respond with ONLY a JSON object, no markdown fences:
  {"reply": "<what you did or found, concise, plain text>",
   "steps": <full replacement steps array, or null>}

Set "steps" to null when the user asked a question or a review found nothing to
change. When you do return steps, return the COMPLETE array — it replaces the
draft wholesale. Never invent action_set_id/action_id values; use "http" steps
unless the user names a registered action.

Placeholder steps in the draft (e.g. a GET to https://example.com/api) are
editor boilerplate, not user intent: replace them, never keep them. Only
reference template paths a response will actually contain — if you do not know
a response's shape, do not build a step on an invented field; leave it out and
say so in the reply."""


class AssistRequest(BaseModel):
    message: str
    name: str | None = None
    steps: list[dict[str, Any]] | None = None


class AssistResponse(BaseModel):
    reply: str
    steps: list[WorkflowStep] | None = None


# Kept as a local name because this module's tests address it; the rules moved
# to `llm_text` once the action orchestrator needed the same two.
_strip_fences = strip_code_fences


def parse_assist_reply(content: str) -> AssistResponse:
    """Model output → validated response. A malformed or invalid answer becomes
    a plain reply rather than an error — the user can just rephrase."""
    try:
        data = json.loads(_strip_fences(content))
    except json.JSONDecodeError:
        return AssistResponse(reply=content.strip(), steps=None)
    if not isinstance(data, dict):
        return AssistResponse(reply=content.strip(), steps=None)

    reply = str(data.get("reply") or "").strip() or "Done."
    raw_steps = data.get("steps")
    if raw_steps is None:
        return AssistResponse(reply=reply, steps=None)
    try:
        steps = [WorkflowStep(**s) for s in raw_steps]
        validate_steps(steps)
    except (TypeError, ValueError) as e:
        return AssistResponse(
            reply=f"{reply}\n\n(The suggested steps were invalid and were discarded: {e})",
            steps=None,
        )
    return AssistResponse(reply=reply, steps=steps)


async def assist(req: AssistRequest) -> AssistResponse:
    user_parts = [req.message.strip()]
    if req.name:
        user_parts.append(f"Workflow name: {req.name}")
    if req.steps is not None:
        user_parts.append("Current steps:\n" + json.dumps(req.steps, indent=2))
    content, _tokens, _cost = await call_llm(
        [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": "\n\n".join(user_parts)},
        ],
        require_json=True,
        temperature=0.2,
    )
    return parse_assist_reply(content)
