"""generate_planner_dag retries and optional PLANNER_FALLBACK_MODEL on last attempt."""

import asyncio
import json

import pytest


def _minimal_dag_json() -> str:
    return json.dumps(
        {
            "team_name": "T",
            "goal_restatement": "G",
            "subagents": [
                {
                    "client_uuid": "a",
                    "role": "R1",
                    "system_prompt": "S",
                    "instructions": "I",
                    "dependencies": [],
                }
            ],
        }
    )


def test_generate_planner_dag_retries_on_bad_json(monkeypatch):
    calls: list[str | None] = []

    async def fake_call_llm(messages, model=None, require_json=False, temperature=0.7):
        calls.append(model)
        if len(calls) == 1:
            return "not valid json {", 5, 0.0
        return _minimal_dag_json(), 10, 0.0

    monkeypatch.setattr("llm_client.call_llm", fake_call_llm)
    monkeypatch.setenv("PLANNER_MODEL", "local")
    monkeypatch.delenv("PLANNER_FALLBACK_MODEL", raising=False)
    monkeypatch.setenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS", "3")

    from llm_client import generate_planner_dag

    dag, tokens, cost = asyncio.run(generate_planner_dag("do something"))
    assert dag["team_name"] == "T"
    assert tokens == 15
    assert cost == 0.0
    assert len(calls) == 2
    assert calls[0] == "local"
    assert calls[1] == "local"


def test_generate_planner_dag_last_attempt_uses_fallback(monkeypatch):
    calls: list[str | None] = []

    async def fake_call_llm(messages, model=None, require_json=False, temperature=0.7):
        calls.append(model)
        if len(calls) == 1:
            return "not json", 1, 0.0
        return _minimal_dag_json(), 2, 0.0

    monkeypatch.setattr("llm_client.call_llm", fake_call_llm)
    monkeypatch.setenv("PLANNER_MODEL", "local")
    monkeypatch.setenv("PLANNER_FALLBACK_MODEL", "strong")
    monkeypatch.setenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS", "2")

    from llm_client import generate_planner_dag

    asyncio.run(generate_planner_dag("goal"))
    assert calls == ["local", "strong"]


def test_generate_planner_dag_raises_after_exhausted(monkeypatch):
    async def bad_call_llm(messages, model=None, require_json=False, temperature=0.7):
        return "xxx", 0, 0.0

    monkeypatch.setattr("llm_client.call_llm", bad_call_llm)
    monkeypatch.setenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS", "2")
    monkeypatch.delenv("PLANNER_FALLBACK_MODEL", raising=False)

    from llm_client import generate_planner_dag

    with pytest.raises(ValueError, match="Planner failed after 2 attempt"):
        asyncio.run(generate_planner_dag("g"))


def test_generate_planner_dag_single_attempt_no_retry(monkeypatch):
    async def bad_call_llm(messages, model=None, require_json=False, temperature=0.7):
        return "not-json", 1, 0.0

    monkeypatch.setattr("llm_client.call_llm", bad_call_llm)
    monkeypatch.setenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS", "1")

    from llm_client import generate_planner_dag

    with pytest.raises(ValueError, match="Planner failed after 1 attempt"):
        asyncio.run(generate_planner_dag("g"))


def test_generate_planner_dag_appends_team_context_to_user_message(monkeypatch):
    captured: list[list] = []

    async def fake_call_llm(messages, model=None, require_json=False, temperature=0.7):
        captured.append(messages)
        return _minimal_dag_json(), 1, 0.0

    monkeypatch.setattr("llm_client.call_llm", fake_call_llm)
    monkeypatch.setenv("PLANNER_MODEL", "local")
    monkeypatch.delenv("PLANNER_FALLBACK_MODEL", raising=False)
    monkeypatch.setenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS", "3")

    from llm_client import generate_planner_dag

    asyncio.run(
        generate_planner_dag("my goal", "Preferred team roster:\n- Researcher: digs in")
    )
    assert len(captured) == 1
    user_content = captured[0][1]["content"]
    assert "Goal: my goal" in user_content
    assert "Preferred team roster:" in user_content
    assert "Researcher" in user_content


def _valid_subagents_only(parent_uuid: str) -> str:
    return json.dumps(
        {
            "subagents": [
                {
                    "client_uuid": "new1",
                    "role": "R",
                    "system_prompt": "S",
                    "instructions": "I",
                    "dependencies": [parent_uuid],
                }
            ],
        }
    )


def test_generate_subdag_expansion_retries_on_bad_json(monkeypatch):
    calls: list[str | None] = []
    parent = "p1"

    async def fake_call_llm(messages, model=None, require_json=False, temperature=0.7):
        calls.append(model)
        if len(calls) == 1:
            return "not valid json {", 5, 0.0
        return _valid_subagents_only(parent), 10, 0.0

    monkeypatch.setattr("llm_client.call_llm", fake_call_llm)
    monkeypatch.setenv("PLANNER_MODEL", "local")
    monkeypatch.delenv("PLANNER_FALLBACK_MODEL", raising=False)
    monkeypatch.setenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS", "3")

    from llm_client import generate_subdag_expansion

    out, tokens, cost = asyncio.run(
        generate_subdag_expansion(
            run_goal="g",
            parent_uuid=parent,
            parent_role="R0",
            parent_output="out",
            existing_uuids={"x"},
        )
    )
    assert len(out) == 1
    assert out[0]["client_uuid"] == "new1"
    assert tokens == 15
    assert cost == 0.0
    assert len(calls) == 2


def test_generate_subdag_expansion_raises_after_exhausted(monkeypatch):
    async def bad_call_llm(messages, model=None, require_json=False, temperature=0.7):
        return "xxx", 0, 0.0

    monkeypatch.setattr("llm_client.call_llm", bad_call_llm)
    monkeypatch.setenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS", "2")
    monkeypatch.delenv("PLANNER_FALLBACK_MODEL", raising=False)

    from llm_client import generate_subdag_expansion

    with pytest.raises(ValueError, match="Sub-decomposition planner failed after 2 attempt"):
        asyncio.run(
            generate_subdag_expansion(
                run_goal="g",
                parent_uuid="p1",
                parent_role="R",
                parent_output="o",
                existing_uuids=set(),
            )
        )
