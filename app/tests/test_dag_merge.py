from dag_schema import PlannerDag, merge_planner_with_new_subagents, validate_planner_dag


def test_merge_planner_with_new_subagents():
    base = validate_planner_dag(
        {
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
            ],
        }
    )
    new_raw = [
        {
            "client_uuid": "b",
            "role": "B",
            "system_prompt": "s2",
            "instructions": "i2",
            "dependencies": ["a"],
        },
    ]
    merged = merge_planner_with_new_subagents(base, new_raw)
    assert isinstance(merged, PlannerDag)
    assert len(merged.subagents) == 2
    assert merged.subagents[1].client_uuid == "b"
