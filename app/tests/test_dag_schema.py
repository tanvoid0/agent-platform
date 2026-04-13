import pytest

from dag_schema import PlannerDag, validate_planner_dag


def _minimal_raw(**overrides):
    base = {
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
    base.update(overrides)
    return base


def test_valid_linear_chain():
    raw = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
            },
            {
                "client_uuid": "b",
                "role": "B",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": ["a"],
            },
        ],
    }
    dag = validate_planner_dag(raw)
    assert isinstance(dag, PlannerDag)
    assert len(dag.subagents) == 2


def test_duplicate_client_uuid():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "x",
                "role": "A",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
            },
            {
                "client_uuid": "x",
                "role": "B",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
            },
        ]
    )
    with pytest.raises(ValueError, match="Duplicate client_uuid"):
        validate_planner_dag(raw)


def test_unknown_dependency():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": ["missing"],
            },
        ]
    )
    with pytest.raises(ValueError, match="Unknown dependency"):
        validate_planner_dag(raw)


def test_cycle_two_nodes():
    raw = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": ["b"],
            },
            {
                "client_uuid": "b",
                "role": "B",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": ["a"],
            },
        ],
    }
    with pytest.raises(ValueError, match="cycle"):
        validate_planner_dag(raw)


def test_self_cycle():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": ["a"],
            },
        ]
    )
    with pytest.raises(ValueError, match="cycle"):
        validate_planner_dag(raw)


def test_empty_subagents_rejected():
    raw = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [],
    }
    with pytest.raises(ValueError, match="Invalid planner DAG"):
        validate_planner_dag(raw)


def test_optional_model_alias():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
                "model": "local",
            },
        ]
    )
    dag = validate_planner_dag(raw)
    assert dag.subagents[0].llm_model == "local"


def test_planner_skill_slug_model_stripped():
    """Planners sometimes emit role-style slugs (typescript-expert) in `model` by mistake."""
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "TypeScript specialist",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
                "model": "typescript-expert",
            },
        ]
    )
    dag = validate_planner_dag(raw)
    assert dag.subagents[0].llm_model is None


def test_planner_skill_slug_model_stripped_case_insensitive():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "TS",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
                "model": "TypeScript-Expert",
            },
        ]
    )
    dag = validate_planner_dag(raw)
    assert dag.subagents[0].llm_model is None


def test_planner_skill_slug_unicode_hyphen_stripped():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "TS",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
                "model": "typescript\u2011expert",
            },
        ]
    )
    dag = validate_planner_dag(raw)
    assert dag.subagents[0].llm_model is None


def test_planner_react_scaffolder_slug_stripped():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "Scaffolder",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
                "model": "react-scaffolder",
            },
        ]
    )
    dag = validate_planner_dag(raw)
    assert dag.subagents[0].llm_model is None


def test_gemini_flash_model_kept():
    raw = _minimal_raw(
        subagents=[
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "s",
                "instructions": "i",
                "dependencies": [],
                "model": "gemini-flash",
            },
        ]
    )
    dag = validate_planner_dag(raw)
    assert dag.subagents[0].llm_model == "gemini-flash"
