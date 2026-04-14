from models import Process, TaskNode
from services.planner_runtime_service import (
    apply_planner_failure,
    apply_planner_success,
    mark_process_planning,
)
from sqlmodel import Session, select


def _seed_process(test_engine) -> int:
    with Session(test_engine) as session:
        proc = Process(goal="g", status="pending")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        return proc.id


def test_mark_process_planning(test_engine):
    pid = _seed_process(test_engine)
    with Session(test_engine) as session:
        run = mark_process_planning(session, process_id=pid)
        assert run.status == "planning"


def test_apply_planner_success_sets_approval_and_tasks(test_engine):
    pid = _seed_process(test_engine)
    dag = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [
            {
                "client_uuid": "a",
                "role": "R",
                "system_prompt": "S",
                "instructions": "I",
                "dependencies": [],
            }
        ],
    }
    with Session(test_engine) as session:
        run = apply_planner_success(
            session,
            process_id=pid,
            dag=dag,
            tokens=9,
            plan_cost=0.4,
        )
        assert run.status == "approval_required"
        assert run.total_tokens == 9
        assert abs(run.total_cost - 0.4) < 1e-9

    with Session(test_engine) as session:
        tasks = session.exec(select(TaskNode).where(TaskNode.process_id == pid)).all()
        assert len(tasks) == 1
        assert tasks[0].client_uuid == "a"


def test_apply_planner_failure_sets_failed(test_engine):
    pid = _seed_process(test_engine)
    with Session(test_engine) as session:
        run = apply_planner_failure(session, process_id=pid, reason="oops")
        assert run.status == "failed"
        assert run.failure_reason == "oops"
