import json

from dag_schema import validate_planner_dag
from models import Process, TaskNode
from services.subdag_service import merge_and_persist_subdag_expansion
from sqlmodel import Session, select


def test_merge_and_persist_subdag_expansion_creates_tasks(test_engine):
    process_id = None
    with Session(test_engine) as session:
        proc = Process(goal="goal", status="running", dag_json=json.dumps({"team_name": "T", "goal_restatement": "G", "subagents": [{"client_uuid": "a", "role": "A", "system_prompt": "s", "instructions": "i", "dependencies": []}]}))
        session.add(proc)
        session.commit()
        session.refresh(proc)
        process_id = proc.id
        session.add(
            TaskNode(
                process_id=proc.id,
                client_uuid="a",
                role="A",
                system_prompt="s",
                instructions="i",
                status="completed",
            )
        )
        session.commit()

        planner = validate_planner_dag(
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
                    }
                ],
            }
        )
        created = merge_and_persist_subdag_expansion(
            session,
            process_id=proc.id,
            planner=planner,
            new_raw=[
                {
                    "client_uuid": "child-1",
                    "role": "Child",
                    "system_prompt": "cs",
                    "instructions": "ci",
                    "dependencies": ["a"],
                }
            ],
            add_tokens=5,
            add_cost=0.2,
            parent_uuid="a",
        )
        assert created == 1

    with Session(test_engine) as session:
        proc = session.get(Process, process_id)
        assert proc.total_tokens == 5
        assert abs(proc.total_cost - 0.2) < 1e-9
        tasks = session.exec(select(TaskNode).where(TaskNode.process_id == proc.id)).all()
        assert any(t.client_uuid == "child-1" for t in tasks)
