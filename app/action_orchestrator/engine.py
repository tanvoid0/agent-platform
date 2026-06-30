"""AI decision engine for action orchestration."""

from __future__ import annotations

import json
import logging
from typing import Any

from action_orchestrator.models import Action
from action_orchestrator.schemas import PlannedAction
from llm_client import call_llm, call_llm_tool_proposals

logger = logging.getLogger(__name__)


def build_action_tools(actions: list[Action]) -> list[dict[str, Any]]:
    """Convert actions to OpenAI-compatible tool definitions."""
    tools = []
    for action in actions:
        params = action.get_parameters()
        tool = {
            "type": "function",
            "function": {
                "name": action.action_id,
                "description": action.description,
                "parameters": params if params else {"type": "object", "properties": {}},
            },
        }
        tools.append(tool)
    return tools


def build_system_message() -> str:
    """Build the system message for the AI decision engine."""
    return """You are an intelligent action planner. Your job is to analyze the user's goal and context, then select the appropriate actions from the available tools to accomplish that goal.

Guidelines:
1. Analyze the goal and context carefully
2. Select only actions that are relevant and necessary
3. Set appropriate parameters for each action based on the context
4. If multiple actions are needed, they will be called in sequence
5. If no action is appropriate, indicate completion
6. Provide clear reasoning for your choices
7. For ask_clarifying_questions, always pass a non-empty "questions" array of specific strings
   (never call it with empty parameters). Optionally pass "fields" with id, label, kind
   (boolean | single_select | multi_select | text | textarea), options for selects, and required.
   Put choices in parentheses in the question or in options — the UI will show pickers.
   If user_domain_profiles already has the needed fields, prefer create_item / break_down_task
   instead of asking again.

You can call multiple tools if needed to accomplish complex goals."""


def build_user_message(
    goal: str,
    context: dict[str, Any],
    history: list[dict[str, Any]] | None = None,
) -> str:
    """Build the user message with goal, context, and optional history."""
    parts = [f"Goal: {goal}"]

    if context:
        conv = context.get("conversation_history")
        ctx_for_json = {k: v for k, v in context.items() if k != "conversation_history"}
        parts.append(f"Context: {json.dumps(ctx_for_json, indent=2)}")
        if isinstance(conv, list) and conv:
            parts.append("\nConversation so far:")
            for turn in conv[-12:]:
                if not isinstance(turn, dict):
                    continue
                role = turn.get("role", "user")
                text = turn.get("content", "")
                if text:
                    parts.append(f"{role}: {text}")

    if history:
        parts.append("\nPrevious actions and results:")
        for h in history:
            action_id = h.get("action_id", "unknown")
            result = h.get("result", {})
            error = h.get("error")
            if error:
                parts.append(f"- {action_id}: FAILED - {error}")
            else:
                parts.append(f"- {action_id}: {json.dumps(result, indent=2)[:200]}")

    return "\n\n".join(parts)


async def decide_actions(
    goal: str,
    context: dict[str, Any],
    actions: list[Action],
    history: list[dict[str, Any]] | None = None,
    llm_model: str | None = None,
) -> tuple[list[PlannedAction], str | None]:
    """Use AI to decide which actions to execute.

    Returns:
        Tuple of (planned_actions, thought/reasoning)
    """
    if not actions:
        return [], "No actions available in the action set."

    tools = build_action_tools(actions)
    system_msg = build_system_message()
    user_msg = build_user_message(goal, context, history)

    messages = [
        {"role": "system", "content": system_msg},
        {"role": "user", "content": user_msg},
    ]

    action_by_id = {a.action_id: a for a in actions}

    try:
        content, raw_tool_calls, _, _ = await call_llm_tool_proposals(
            messages,
            model=llm_model,
            tools=tools,
            temperature=0.7,
        )

        planned_actions = tool_calls_to_planned_actions(raw_tool_calls, action_by_id)
        if planned_actions:
            thought = content.strip() or None
            return planned_actions, thought

        # Fallback: parse structured tags or action lines from plain text
        parsed = parse_decision_response(content or "")
        if parsed["actions"]:
            return parsed["actions"], parsed["thought"]

        reasoning_response, _, _ = await call_llm(messages, model=llm_model)
        parsed = parse_decision_response(reasoning_response or "")
        return parsed["actions"], parsed["thought"]

    except Exception as e:
        logger.exception("Error in decide_actions")
        return [], f"Error during decision: {str(e)}"


def tool_calls_to_planned_actions(
    raw_tool_calls: list[dict[str, Any]],
    action_by_id: dict[str, Action],
) -> list[PlannedAction]:
    """Convert OpenAI-style tool_calls into planned actions (not executed)."""
    planned: list[PlannedAction] = []
    for tc in raw_tool_calls:
        fn = tc.get("function") or {}
        action_id = (fn.get("name") or "").strip()
        if not action_id or action_id not in action_by_id:
            continue
        args_raw = fn.get("arguments")
        if isinstance(args_raw, str):
            try:
                parameters = json.loads(args_raw) if args_raw.strip() else {}
            except json.JSONDecodeError:
                parameters = {}
        elif isinstance(args_raw, dict):
            parameters = args_raw
        else:
            parameters = {}
        action = action_by_id[action_id]
        planned.append(
            PlannedAction(
                action_id=action_id,
                name=action.name,
                parameters=parameters,
                confidence=0.9,
            )
        )
    return planned


def parse_decision_response(response: str) -> dict[str, Any]:
    """Parse the LLM response for actions and reasoning."""
    thought = None
    actions: list[PlannedAction] = []

    # Try to extract reasoning
    if "<reasoning>" in response and "</reasoning>" in response:
        start = response.index("<reasoning>") + len("<reasoning>")
        end = response.index("</reasoning>")
        thought = response[start:end].strip()
    elif "Thought:" in response:
        # Try alternate format
        lines = response.split("\n")
        for i, line in enumerate(lines):
            if line.startswith("Thought:"):
                thought = line.replace("Thought:", "").strip()
                # Collect following lines until we hit Actions or end
                for j in range(i + 1, len(lines)):
                    if lines[j].strip().startswith("Action"):
                        break
                    thought += "\n" + lines[j]
                break

    # Try to extract actions as JSON
    if "<actions>" in response and "</actions>" in response:
        try:
            start = response.index("<actions>") + len("<actions>")
            end = response.index("</actions>")
            actions_json = response[start:end].strip()
            actions_data = json.loads(actions_json)
            for a in actions_data:
                actions.append(PlannedAction(
                    action_id=a.get("action_id", ""),
                    name=a.get("name", ""),
                    parameters=a.get("parameters", {}),
                    confidence=a.get("confidence", 0.9),
                    reasoning=a.get("reasoning"),
                ))
        except (json.JSONDecodeError, ValueError) as e:
            logger.warning(f"Failed to parse actions from response: {e}")

    # If no structured actions found, try to parse from text
    if not actions:
        actions = parse_actions_from_text(response)

    return {"thought": thought or response[:200], "actions": actions}


def parse_actions_from_text(text: str) -> list[PlannedAction]:
    """Fallback parser to extract actions from unstructured text."""
    actions = []

    # Look for patterns like "Action: action_name" or numbered lists
    lines = text.split("\n")
    current_action = None

    for line in lines:
        line = line.strip()
        if not line:
            continue

        # Try to identify action mentions
        if line.lower().startswith("action:") or line.lower().startswith("- action:"):
            if current_action:
                actions.append(current_action)
            action_name = line.split(":", 1)[1].strip().split()[0]
            current_action = PlannedAction(
                action_id=action_name,
                name=action_name,
                parameters={},
                confidence=0.8,
            )
        elif current_action and "=" in line:
            # Parse parameter assignments
            try:
                key, value = line.split("=", 1)
                key = key.strip()
                value = value.strip().strip('"\'')
                current_action.parameters[key] = value
            except ValueError:
                pass

    if current_action:
        actions.append(current_action)

    return actions
