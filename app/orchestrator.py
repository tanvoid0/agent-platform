import asyncio
import logging
from sqlmodel import Session, select
from database import engine
from models import Run, TaskNode, EventLog
from llm_client import (
    call_llm,
    generate_planner_dag,
    LLMAuthenticationError,
    LLMConfigurationError,
    LLMRequestError,
    LLMTransportError,
)
from datetime import datetime

class DAGExecutor:
    def __init__(self, run_id: int):
        self.run_id = run_id

    def log_event(self, session: Session, event_type: str, content: str, task_id: int = None):
        log = EventLog(run_id=self.run_id, task_id=task_id, event_type=event_type, content=content)
        session.add(log)
        session.commit()

    async def plan(self, goal: str):
        # Generates the planner DAG and saves tasks to DB
        with Session(engine) as session:
            run = session.get(Run, self.run_id)
            run.status = "planning"
            session.commit()
            self.log_event(session, "status_change", "Run status updated to planning")

        try:
            dag, tokens = await generate_planner_dag(goal)
            
            with Session(engine) as session:
                run = session.get(Run, self.run_id)
                run.dag_json = str(dag)
                run.total_tokens += tokens
                run.status = "approval_required" # Human in the loop gate
                
                for agent in dag.get("subagents", []):
                    task = TaskNode(
                        run_id=self.run_id,
                        client_uuid=agent["client_uuid"],
                        role=agent["role"],
                        system_prompt=agent["system_prompt"],
                        instructions=agent["instructions"],
                    )
                    task.dependencies = agent.get("dependencies", [])
                    session.add(task)
                
                session.commit()
                self.log_event(session, "status_change", "Run requires approval to execute DAG")
                
        except (
            LLMConfigurationError,
            LLMAuthenticationError,
            LLMTransportError,
            LLMRequestError,
            ValueError,
        ) as e:
            with Session(engine) as session:
                run = session.get(Run, self.run_id)
                run.status = "failed"
                session.commit()
                self.log_event(session, "error", f"Planning failed: {e}")
        except Exception:
            logging.exception("Planning failed (unexpected)")
            with Session(engine) as session:
                run = session.get(Run, self.run_id)
                run.status = "failed"
                session.commit()
                self.log_event(session, "error", "Planning failed: unexpected error")

    async def execute_task(self, task_id: int):
        with Session(engine) as session:
            task = session.get(TaskNode, task_id)
            task.status = "running"
            task.started_at = datetime.utcnow()
            session.commit()
            self.log_event(session, "status_change", f"Task {task.client_uuid} started executing", task.id)
            
            # Re-fetch dependencies forming the "Blackboard" / edges output context
            deps_texts = []
            if task.dependencies:
                dep_tasks = session.exec(
                    select(TaskNode)
                    .where(TaskNode.run_id == self.run_id)
                    .where(TaskNode.client_uuid.in_(task.dependencies))
                ).all()
                for dt in dep_tasks:
                    deps_texts.append(f"Output from {dt.client_uuid} ({dt.role}):\n{dt.output}")

        system_message = task.system_prompt
        user_message = task.instructions
        if deps_texts:
            user_message += "\n\nContext from previous steps:\n" + "\n---\n".join(deps_texts)

        try:
            content, tokens = await call_llm([
                {"role": "system", "content": system_message},
                {"role": "user", "content": user_message}
            ])
            
            with Session(engine) as session:
                task = session.get(TaskNode, task_id)
                task.output = content
                task.tokens_used = tokens
                task.status = "completed"
                task.completed_at = datetime.utcnow()
                
                run = session.get(Run, self.run_id)
                run.total_tokens += tokens
                
                session.commit()
                self.log_event(session, "status_change", f"Task {task.client_uuid} completed", task.id)
                self.log_event(session, "trace", content, task.id) # Trace LLM output
                
        except (
            LLMConfigurationError,
            LLMAuthenticationError,
            LLMTransportError,
            LLMRequestError,
        ) as e:
            with Session(engine) as session:
                task = session.get(TaskNode, task_id)
                task.status = "failed"
                run = session.get(Run, self.run_id)
                run.status = "failed"
                session.commit()
                self.log_event(
                    session,
                    "error",
                    f"Task {task.client_uuid} failed: {e}",
                    task.id,
                )
        except Exception:
            logging.exception("Task %s failed (unexpected)", task_id)
            with Session(engine) as session:
                task = session.get(TaskNode, task_id)
                task.status = "failed"
                run = session.get(Run, self.run_id)
                run.status = "failed"
                session.commit()
                self.log_event(
                    session,
                    "error",
                    f"Task {task.client_uuid} failed: unexpected error",
                    task.id,
                )

    async def execute_dag(self):
        # Iterative layer-by-layer topological execution
        with Session(engine) as session:
            run = session.get(Run, self.run_id)
            run.status = "running"
            session.commit()
            self.log_event(session, "status_change", "Run status updated to running")

        while True:
            with Session(engine) as session:
                run = session.get(Run, self.run_id)
                if run.status == "failed":
                    break
                    
                pending_tasks = session.exec(
                    select(TaskNode).where(TaskNode.run_id == self.run_id).where(TaskNode.status == "pending")
                ).all()
                
                completed_tasks = session.exec(
                    select(TaskNode).where(TaskNode.run_id == self.run_id).where(TaskNode.status == "completed")
                ).all()
                completed_uuids = {t.client_uuid for t in completed_tasks}

            if not pending_tasks:
                with Session(engine) as session:
                    run = session.get(Run, self.run_id)
                    run.status = "completed"
                    session.commit()
                    self.log_event(session, "status_change", "Run execution fully completed")
                break

            # Find tasks where all dependencies are met
            ready_tasks = []
            for task in pending_tasks:
                if all(dep in completed_uuids for dep in task.dependencies):
                    ready_tasks.append(task)

            if not ready_tasks:
                # Deadlock detection if there are pending tasks but none are ready
                with Session(engine) as session:
                    run = session.get(Run, self.run_id)
                    run.status = "failed"
                    session.commit()
                    self.log_event(session, "error", "DAG deadlock detected. Unmet cyclic dependencies.")
                break

            # Execute ready tasks in parallel sprints (GPT-Researcher Style Split)
            await asyncio.gather(*(self.execute_task(t.id) for t in ready_tasks))
