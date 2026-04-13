import asyncio
import json
import logging
import os
import time
import traceback
from datetime import datetime
from typing import Any

from sqlmodel import Session, select

from dag_schema import (
    merge_planner_with_new_subagents,
    planner_dag_to_json_dict,
    sanitize_llm_model_alias,
    validate_planner_dag,
)
from database import engine
from context_budget import dependency_context_token_budget, fit_dependency_outputs_to_budget
from context_summarize import maybe_condense_text_for_context
from llm_client import (
    LLMAuthenticationError,
    LLMConfigurationError,
    LLMRequestError,
    LLMTransportError,
    call_llm,
    call_llm_with_tools,
    generate_planner_dag,
    generate_subdag_expansion,
)
from models import EventLog, Process, TaskNode
from process_approval import apply_validated_planner_to_process
from tool_context import ToolContext
from tools_policy import load_policy


def _plan_timeout_seconds() -> float | None:
    raw = (os.getenv("AGENT_PLATFORM_PLAN_TIMEOUT_SECONDS") or "").strip()
    if not raw:
        return None
    try:
        v = float(raw)
        return v if v > 0 else None
    except ValueError:
        return None


def _subdecomp_max_expansions() -> int:
    # Default > 0 so planner `subdecompose` nodes can spawn follow-on work without extra env.
    raw = (os.getenv("AGENT_PLATFORM_SUBDECOMP_MAX_EXPANSIONS") or "48").strip()
    try:
        return max(0, int(raw))
    except ValueError:
        return 0


def _subdecomp_max_new_tasks() -> int:
    raw = (os.getenv("AGENT_PLATFORM_SUBDECOMP_MAX_NEW_TASKS") or "48").strip()
    try:
        return max(0, int(raw))
    except ValueError:
        return 0


def _subdecomp_max_depth() -> int | None:
    """
    Optional cap on expansion depth: tasks added by an expansion have depth parent+1;
    planner tasks are depth 0. Unset or non-positive = unlimited.
    """
    raw = (os.getenv("AGENT_PLATFORM_SUBDECOMP_MAX_DEPTH") or "").strip()
    if not raw:
        return None
    try:
        v = int(raw)
        return v if v > 0 else None
    except ValueError:
        return None


def _run_max_seconds() -> float | None:
    raw = (os.getenv("AGENT_PLATFORM_RUN_MAX_SECONDS") or "").strip()
    if not raw:
        return None
    try:
        v = float(raw)
        return v if v > 0 else None
    except ValueError:
        return None


def _max_concurrent_tasks() -> int | None:
    """
    Cap how many dependency-ready tasks run at once. Unset or non-positive = unlimited (legacy: all ready in parallel).
    Ready tasks are started in ascending TaskNode.id order (FIFO). Free capacity picks the next id, like a worker pool.
    """
    raw = (os.getenv("AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS") or "").strip()
    if not raw:
        return None
    try:
        v = int(raw)
        return v if v > 0 else None
    except ValueError:
        return None


def _env_auto_approve() -> bool:
    raw = (os.getenv("AGENT_PLATFORM_AUTO_APPROVE") or "").strip().lower()
    return raw in ("1", "true", "yes", "on")


def _truncate_reason(msg: str, max_len: int = 2048) -> str:
    msg = str(msg).strip()
    if len(msg) <= max_len:
        return msg
    return msg[: max_len - 3] + "..."


def _task_failure_debug_json(
    *,
    source: str,
    exc: BaseException | None = None,
    extra: dict[str, Any] | None = None,
) -> str:
    body: dict[str, Any] = {"source": source}
    if exc is not None:
        body["exception_type"] = type(exc).__name__
        body["message"] = str(exc)
    if extra:
        body.update(extra)
    raw = json.dumps(body, ensure_ascii=False)
    max_len = 16000
    if len(raw) <= max_len:
        return raw
    return raw[: max_len - 3] + "..."


def _revision_user_preamble(task: TaskNode) -> str:
    """Extra user message when re-running after request_changes."""
    parts: list[str] = []
    if task.draft_output:
        parts.append(f"Previous attempt:\n{task.draft_output}")
    if task.review_feedback:
        parts.append(f"Reviewer feedback:\n{task.review_feedback}")
    if not parts:
        return ""
    return "\n\n---\n".join(parts) + "\n\nRevise your output to address the feedback above.\n\n"


def _deps_all_completed_for_task(t: TaskNode, tasks_by_uuid: dict[str, TaskNode]) -> bool:
    for dep in t.dependencies:
        dep_row = tasks_by_uuid.get(dep)
        if not dep_row or dep_row.status != "completed":
            return False
    return True


def _can_serve_as_reviewer(rt: TaskNode, tasks_by_uuid: dict[str, TaskNode]) -> bool:
    """True when this peer is idle enough to take a review (not the author’s runnable work)."""
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
    subagents: list,
    tasks_by_uuid: dict[str, TaskNode],
) -> str | None:
    """
    Choose a peer subagent to “hold” the review: prefers downstream consumers of the author’s
    output, then role overlap; only considers idle peers (completed, or pending but blocked).
    """
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
    """Fill or refresh reviewer_client_uuid on tasks in awaiting_review."""
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


class DAGExecutor:
    def __init__(self, process_id: int, auto_approve: bool = False):
        self.process_id = process_id
        self._auto_approve = auto_approve
        self._execution_deadline: float | None = None
        self._merge_lock = asyncio.Lock()
        self._subdecomp_expansions_used: int = 0
        self._subdecomp_new_tasks_added: int = 0
        # client_uuid -> depth of tasks introduced by sub-DAG expansion (planner tasks default to 0)
        self._subdecomp_uuid_depth: dict[str, int] = {}

    def _should_auto_approve(self) -> bool:
        return self._auto_approve or _env_auto_approve()

    def log_event(self, session: Session, event_type: str, content: str, task_id: int = None):
        log = EventLog(process_id=self.process_id, task_id=task_id, event_type=event_type, content=content)
        session.add(log)
        session.commit()

    async def plan(self, goal: str, team_context: str | None = None):
        # Generates the planner DAG and saves tasks to DB
        with Session(engine) as session:
            run = session.get(Process, self.process_id)
            run.status = "planning"
            session.commit()
            self.log_event(session, "status_change", "Process status updated to planning")

        plan_timeout = _plan_timeout_seconds()
        try:
            if plan_timeout is not None:
                dag, tokens, plan_cost = await asyncio.wait_for(
                    generate_planner_dag(goal, team_context),
                    timeout=plan_timeout,
                )
            else:
                dag, tokens, plan_cost = await generate_planner_dag(goal, team_context)

            with Session(engine) as session:
                run = session.get(Process, self.process_id)
                run.dag_json = json.dumps(dag)
                run.total_tokens += tokens
                run.total_cost += plan_cost
                run.failure_reason = None
                run.status = "approval_required" # Human in the loop gate
                
                for agent in dag.get("subagents", []):
                    task = TaskNode(
                        process_id=self.process_id,
                        client_uuid=agent["client_uuid"],
                        role=agent["role"],
                        system_prompt=agent["system_prompt"],
                        instructions=agent["instructions"],
                        llm_model=sanitize_llm_model_alias(agent.get("model")),
                        requires_review=bool(agent.get("requires_review", False)),
                    )
                    task.dependencies = agent.get("dependencies", [])
                    session.add(task)
                
                session.commit()
                self.log_event(session, "status_change", "Process requires approval to execute DAG")

            if self._should_auto_approve():
                await self._auto_approve_and_execute()

        except asyncio.TimeoutError:
            reason = (
                "Planning exceeded AGENT_PLATFORM_PLAN_TIMEOUT_SECONDS"
                if plan_timeout is not None
                else "Planning timed out"
            )
            with Session(engine) as session:
                run = session.get(Process, self.process_id)
                run.status = "failed"
                run.failure_reason = reason
                session.commit()
                self.log_event(session, "error", f"Planning failed: {reason}")
        except (
            LLMConfigurationError,
            LLMAuthenticationError,
            LLMTransportError,
            LLMRequestError,
            ValueError,
        ) as e:
            with Session(engine) as session:
                run = session.get(Process, self.process_id)
                run.status = "failed"
                run.failure_reason = _truncate_reason(f"Planning failed: {e}")
                session.commit()
                self.log_event(session, "error", f"Planning failed: {e}")
        except Exception:
            logging.exception("Planning failed (unexpected)")
            with Session(engine) as session:
                run = session.get(Process, self.process_id)
                run.status = "failed"
                run.failure_reason = "Planning failed: unexpected error"
                session.commit()
                self.log_event(session, "error", "Planning failed: unexpected error")

    async def _auto_approve_and_execute(self) -> None:
        with Session(engine) as session:
            run = session.get(Process, self.process_id)
            if not run or run.status != "approval_required" or not run.dag_json:
                return
            try:
                raw = json.loads(run.dag_json)
                validated = validate_planner_dag(raw)
            except (json.JSONDecodeError, ValueError) as e:
                logging.warning("Auto-approve skipped (invalid DAG): %s", e)
                with Session(engine) as session_err:
                    self.log_event(session_err, "error", f"Auto-approve skipped: {e}")
                return
            apply_validated_planner_to_process(session, self.process_id, validated)
            run = session.get(Process, self.process_id)
            run.status = "approved"
            session.commit()
            self.log_event(session, "status_change", "Process auto-approved; scheduling execution")
        await self.execute_dag()

    async def execute_task(self, task_id: int):
        with Session(engine) as session:
            task = session.get(TaskNode, task_id)
            if not task:
                return
            task.status = "running"
            task.started_at = datetime.utcnow()
            task.failure_debug_json = None
            session.commit()
            self.log_event(session, "status_change", f"Task {task.client_uuid} started executing", task.id)

            deps_texts: list[str] = []
            if task.dependencies:
                dep_tasks = session.exec(
                    select(TaskNode)
                    .where(TaskNode.process_id == self.process_id)
                    .where(TaskNode.client_uuid.in_(task.dependencies))
                ).all()
                for dt in dep_tasks:
                    deps_texts.append(
                        f"Output from {dt.client_uuid} ({dt.role}):\n{dt.output or ''}"
                    )

            system_message = task.system_prompt
            user_message = task.instructions
            if task.parent_client_uuid:
                parent_row = session.exec(
                    select(TaskNode)
                    .where(TaskNode.process_id == self.process_id)
                    .where(TaskNode.client_uuid == task.parent_client_uuid)
                ).first()
                if parent_row:
                    user_message = (
                        f"This is a subtask spawned after `{parent_row.role}` "
                        f"({task.parent_client_uuid}) completed. Deliver one focused outcome.\n\n"
                        + user_message
                    )
            llm_model = task.llm_model
            client_uuid = task.client_uuid

        rev_pre = _revision_user_preamble(task)
        if rev_pre:
            user_message = rev_pre + user_message

        if deps_texts:
            dep_budget = dependency_context_token_budget(
                system_message=system_message or "",
                instructions_and_preamble=user_message,
            )
            condensed: list[str] = []
            for chunk in deps_texts:
                condensed.append(
                    await maybe_condense_text_for_context(chunk, model=llm_model)
                )
            fitted_deps = fit_dependency_outputs_to_budget(condensed, dep_budget)
            user_message += "\n\nContext from previous steps:\n" + "\n---\n".join(fitted_deps)

        base_messages = [
            {"role": "system", "content": system_message},
            {"role": "user", "content": user_message},
        ]

        try:
            policy = load_policy()
            with Session(engine) as session:
                run = session.get(Process, self.process_id)
                used_tools = int(run.tool_invocations_used or 0) if run else 0
                proj_id: int | None = (
                    int(run.project_id) if run and run.project_id is not None else None
                )
            remaining_tool_budget = 0
            if policy.enabled and policy.allowlist and policy.budget_per_run > 0:
                remaining_tool_budget = max(0, policy.budget_per_run - used_tools)

            if remaining_tool_budget > 0:
                tool_ctx = ToolContext(process_id=self.process_id, project_id=proj_id)
                content, tokens, task_cost, tool_calls = await call_llm_with_tools(
                    base_messages,
                    model=llm_model,
                    allowed_tool_names=policy.allowlist,
                    tool_budget=remaining_tool_budget,
                    temperature=0.7,
                    tool_context=tool_ctx,
                )
            else:
                content, tokens, task_cost = await call_llm(
                    [
                        {"role": "system", "content": system_message},
                        {"role": "user", "content": user_message},
                    ],
                    model=llm_model,
                )
                tool_calls = 0

            needs_expand = False
            with Session(engine) as session:
                task = session.get(TaskNode, task_id)
                task.output = content
                task.tokens_used = tokens

                run = session.get(Process, self.process_id)
                run.total_tokens += tokens
                run.total_cost += task_cost
                run.tool_invocations_used = int(run.tool_invocations_used or 0) + int(tool_calls)

                if task.requires_review:
                    task.status = "awaiting_review"
                    task.completed_at = None
                    run.status = "task_review_required"
                    sync_review_assignments(session, self.process_id)
                    session.commit()
                    self.log_event(
                        session,
                        "status_change",
                        f"Task {client_uuid} awaiting review",
                        task.id,
                    )
                    self.log_event(session, "trace", content, task.id)
                else:
                    task.status = "completed"
                    task.completed_at = datetime.utcnow()
                    needs_expand = True
                    sync_review_assignments(session, self.process_id)
                    session.commit()
                    self.log_event(session, "status_change", f"Task {client_uuid} completed", task.id)
                    self.log_event(session, "trace", content, task.id)  # Trace LLM output

            if needs_expand:
                await self._maybe_expand_subdag_after_success(task_id)

        except (
            LLMConfigurationError,
            LLMAuthenticationError,
            LLMTransportError,
            LLMRequestError,
        ) as e:
            with Session(engine) as session:
                task = session.get(TaskNode, task_id)
                task.status = "failed"
                task.failure_debug_json = _task_failure_debug_json(source="llm", exc=e)
                run = session.get(Process, self.process_id)
                run.status = "failed"
                run.failure_reason = _truncate_reason(f"Task {task.client_uuid} failed: {e}")
                session.commit()
                self.log_event(
                    session,
                    "error",
                    f"Task {task.client_uuid} failed: {e}",
                    task.id,
                )
        except Exception as e:
            logging.exception("Task %s failed (unexpected)", task_id)
            tb = traceback.format_exc()
            with Session(engine) as session:
                task = session.get(TaskNode, task_id)
                task.status = "failed"
                task.failure_debug_json = _task_failure_debug_json(
                    source="unexpected",
                    exc=e,
                    extra={"traceback": _truncate_reason(tb, max_len=12000)},
                )
                run = session.get(Process, self.process_id)
                run.status = "failed"
                run.failure_reason = _truncate_reason(
                    f"Task {task.client_uuid} failed: unexpected error"
                )
                session.commit()
                self.log_event(
                    session,
                    "error",
                    f"Task {task.client_uuid} failed: unexpected error",
                    task.id,
                )

    async def _maybe_expand_subdag_after_success(self, task_id: int) -> None:
        max_exp = _subdecomp_max_expansions()
        max_new = _subdecomp_max_new_tasks()
        if max_exp <= 0 or max_new <= 0:
            return

        async with self._merge_lock:
            with Session(engine) as session:
                task = session.get(TaskNode, task_id)
                run = session.get(Process, self.process_id)
                if not task or not run or not run.dag_json:
                    return
                if run.status != "running":
                    return
                if self._subdecomp_expansions_used >= max_exp:
                    return
                if self._subdecomp_new_tasks_added >= max_new:
                    return

                try:
                    planner = validate_planner_dag(json.loads(run.dag_json))
                except (json.JSONDecodeError, ValueError) as e:
                    logging.warning("Sub-DAG expansion skipped (invalid dag_json): %s", e)
                    return

                spec = next(
                    (s for s in planner.subagents if s.client_uuid == task.client_uuid),
                    None,
                )
                if not spec or not spec.subdecompose or spec.requires_review:
                    return

                max_depth = _subdecomp_max_depth()
                if max_depth is not None:
                    parent_depth = self._subdecomp_uuid_depth.get(task.client_uuid, 0)
                    if parent_depth + 1 > max_depth:
                        return

                run_goal = run.goal
                existing_uuids = {a.client_uuid for a in planner.subagents}
                parent_uuid = task.client_uuid
                parent_role = task.role
                parent_output = task.output or ""
                remaining_slots = max_new - self._subdecomp_new_tasks_added

            try:
                new_raw, add_tokens, add_cost = await generate_subdag_expansion(
                    run_goal=run_goal,
                    parent_uuid=parent_uuid,
                    parent_role=parent_role,
                    parent_output=parent_output,
                    existing_uuids=existing_uuids,
                )
            except Exception as e:
                logging.warning("Sub-DAG expansion LLM failed: %s", e)
                with Session(engine) as session:
                    self.log_event(session, "error", f"Sub-DAG expansion skipped: {e}")
                return

            if remaining_slots <= 0:
                return

            if len(new_raw) > remaining_slots:
                new_raw = new_raw[:remaining_slots]

            try:
                merged = merge_planner_with_new_subagents(planner, new_raw)
            except Exception as e:
                logging.warning("Sub-DAG merge validation failed: %s", e)
                with Session(engine) as session:
                    self.log_event(session, "error", f"Sub-DAG expansion merge failed: {e}")
                return

            merged_json = json.dumps(planner_dag_to_json_dict(merged))

            with Session(engine) as session:
                run = session.get(Process, self.process_id)
                if not run or run.status != "running":
                    return
                run.dag_json = merged_json
                run.total_tokens += add_tokens
                run.total_cost += add_cost

                for agent in new_raw:
                    cu = agent["client_uuid"]
                    exists = session.exec(
                        select(TaskNode)
                        .where(TaskNode.process_id == self.process_id)
                        .where(TaskNode.client_uuid == cu)
                    ).first()
                    if exists:
                        continue
                    tnode = TaskNode(
                        process_id=self.process_id,
                        client_uuid=cu,
                        parent_client_uuid=parent_uuid,
                        role=agent["role"],
                        system_prompt=agent["system_prompt"],
                        instructions=agent["instructions"],
                        llm_model=sanitize_llm_model_alias(agent.get("model")),
                        requires_review=bool(agent.get("requires_review", False)),
                    )
                    tnode.dependencies = agent.get("dependencies", [])
                    session.add(tnode)

                self._subdecomp_expansions_used += 1
                self._subdecomp_new_tasks_added += len(new_raw)
                child_depth = self._subdecomp_uuid_depth.get(parent_uuid, 0) + 1
                for agent in new_raw:
                    cu = agent.get("client_uuid")
                    if isinstance(cu, str) and cu:
                        self._subdecomp_uuid_depth[cu] = child_depth

                session.commit()
                self.log_event(
                    session,
                    "status_change",
                    f"Sub-DAG expansion added {len(new_raw)} task(s) after {parent_uuid}",
                )

    async def execute_dag(self):
        # Iterative layer-by-layer topological execution
        max_run_sec = _run_max_seconds()
        self._execution_deadline = (
            time.monotonic() + max_run_sec if max_run_sec is not None else None
        )

        with Session(engine) as session:
            run = session.get(Process, self.process_id)
            awaiting_left = (
                session.exec(
                    select(TaskNode.id)
                    .where(TaskNode.process_id == self.process_id)
                    .where(TaskNode.status == "awaiting_review")
                ).first()
                is not None
            )
            # Parallel tasks can both land in awaiting_review; approving one resumes execute_dag,
            # which must not report "running" while another review gate is still open.
            run.status = "task_review_required" if awaiting_left else "running"
            run.failure_reason = None
            session.commit()
            self.log_event(
                session,
                "status_change",
                "Process status updated to task_review_required"
                if awaiting_left
                else "Process status updated to running",
            )

        while True:
            with Session(engine) as session:
                run = session.get(Process, self.process_id)
                sync_review_assignments(session, self.process_id)
                session.commit()
                run = session.get(Process, self.process_id)
                if run.status == "cancelled":
                    self.log_event(session, "status_change", "Process stopped (cancelled)")
                    break
                if run.status == "failed":
                    break

                if (
                    self._execution_deadline is not None
                    and time.monotonic() > self._execution_deadline
                ):
                    run.status = "failed"
                    run.failure_reason = (
                        "Process exceeded execution budget (AGENT_PLATFORM_RUN_MAX_SECONDS)"
                    )
                    session.commit()
                    self.log_event(
                        session,
                        "error",
                        run.failure_reason,
                    )
                    break

                pending_tasks = session.exec(
                    select(TaskNode).where(TaskNode.process_id == self.process_id).where(TaskNode.status == "pending")
                ).all()

                awaiting_review_exists = (
                    session.exec(
                        select(TaskNode.id)
                        .where(TaskNode.process_id == self.process_id)
                        .where(TaskNode.status == "awaiting_review")
                    ).first()
                    is not None
                )

                completed_tasks = session.exec(
                    select(TaskNode).where(TaskNode.process_id == self.process_id).where(TaskNode.status == "completed")
                ).all()
                completed_uuids = {t.client_uuid for t in completed_tasks}

            if not pending_tasks:
                if awaiting_review_exists:
                    with Session(engine) as session:
                        run = session.get(Process, self.process_id)
                        run.status = "task_review_required"
                        run.failure_reason = None
                        session.commit()
                        self.log_event(session, "status_change", "Process paused for task review")
                    break
                with Session(engine) as session:
                    run = session.get(Process, self.process_id)
                    run.status = "completed"
                    run.failure_reason = None
                    session.commit()
                    self.log_event(session, "status_change", "Process execution fully completed")
                break

            # Find tasks where all dependencies are met
            ready_tasks = []
            for task in pending_tasks:
                if all(dep in completed_uuids for dep in task.dependencies):
                    ready_tasks.append(task)

            if not ready_tasks:
                if awaiting_review_exists:
                    with Session(engine) as session:
                        run = session.get(Process, self.process_id)
                        run.status = "task_review_required"
                        run.failure_reason = None
                        session.commit()
                        self.log_event(session, "status_change", "Process paused for task review")
                    break
                # Deadlock detection if there are pending tasks but none are ready
                deadlock_msg = (
                    "DAG deadlock: pending tasks with no runnable step "
                    "(cycle or unsatisfied dependencies)"
                )
                with Session(engine) as session:
                    run = session.get(Process, self.process_id)
                    run.status = "failed"
                    run.failure_reason = deadlock_msg
                    session.commit()
                    self.log_event(session, "error", deadlock_msg)
                break

            ready_sorted = sorted(ready_tasks, key=lambda t: t.id)
            max_c = _max_concurrent_tasks()
            batch = ready_sorted if max_c is None else ready_sorted[:max_c]
            await asyncio.gather(*(self.execute_task(t.id) for t in batch))

    async def expand_after_review_approval_and_continue(self, task_id: int) -> None:
        """Sub-DAG expansion after a reviewed task is approved, then continue execution."""
        await self._maybe_expand_subdag_after_success(task_id)
        await self.execute_dag()
