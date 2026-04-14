from __future__ import annotations

import json
from typing import Any

from dag_schema import validate_planner_dag
from models import Process, TaskNode
from sqlmodel import Session, select


def _deps_all_completed_for_task(t: TaskNode, tasks_by_uuid: dict[str, TaskNode]) -> bool:
    for dep in t.dependencies:
        dep_row = tasks_by_uuid.get(dep)
        if not dep_row or dep_row.status != "completed":
            return False
    return True


def _can_serve_as_reviewer(rt: TaskNode, tasks_by_uuid: dict[str, TaskNode]) -> bool:
    if rt.status == "completed":
        return True
    if rt.status != "pending":
        return False
    if not rt.dependencies:
        return False
    return not _deps_all_completed_for_task(rt, tasks_by_uuid)


def _role_word_overlap(r1: str, r2: str) -> int:
    w1 = set(r1.lower().replace("-", " ").split())
    w2 = set(r2.lower().replace("-", " ").split())
    return len(w1 & w2)


def pick_reviewer_client_uuid(
    subject: TaskNode,
    subagents: list[Any],
    tasks_by_uuid: dict[str, TaskNode],
) -> str | None:
    author = subject.client_uuid
    spec_by_uuid = {a.client_uuid: a for a in subagents}
    if author not in spec_by_uuid:
        return None
    author_spec = spec_by_uuid[author]
    downstream = {a.client_uuid for a in subagents if author in (a.dependencies or [])}
    upstream = set(author_spec.dependencies or [])

    scored: list[tuple[int, str]] = []
    for a in subagents:
        cu = a.client_uuid
        if cu == author:
            continue
        rt = tasks_by_uuid.get(cu)
        if not rt or not _can_serve_as_reviewer(rt, tasks_by_uuid):
            continue
        score = 0
        if cu in downstream:
            score += 100
        if author in (a.dependencies or []):
            score += 80
        if cu in upstream:
            score += 40
        score += _role_word_overlap(author_spec.role, a.role)
        scored.append((score, cu))

    if not scored:
        return None
    scored.sort(key=lambda x: (-x[0], x[1]))
    return scored[0][1]


def sync_review_assignments(session: Session, process_id: int) -> None:
    run = session.get(Process, process_id)
    if not run or not run.dag_json:
        return
    try:
        planner = validate_planner_dag(json.loads(run.dag_json))
    except (json.JSONDecodeError, ValueError):
        return
    tasks = session.exec(select(TaskNode).where(TaskNode.process_id == process_id)).all()
    tasks_by_uuid = {t.client_uuid: t for t in tasks}
    subagents = planner.subagents
    for t in tasks:
        if t.status != "awaiting_review":
            continue
        rid = t.reviewer_client_uuid
        if rid:
            rt = tasks_by_uuid.get(rid)
            if not rt or not _can_serve_as_reviewer(rt, tasks_by_uuid):
                t.reviewer_client_uuid = None
                session.add(t)
        if not t.reviewer_client_uuid:
            pick = pick_reviewer_client_uuid(t, subagents, tasks_by_uuid)
            if pick:
                t.reviewer_client_uuid = pick
                session.add(t)
