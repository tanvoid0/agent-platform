import asyncio
import json
import logging
import os
from fastapi import FastAPI, Depends, BackgroundTasks, HTTPException
from fastapi.responses import RedirectResponse
from sqlmodel import Session, select
from sse_starlette.sse import EventSourceResponse
from pydantic import BaseModel

from database import create_db_and_tables, get_session
from models import Run, TaskNode, EventLog
from orchestrator import DAGExecutor

app = FastAPI(title="Agent Platform API")

@app.get("/", include_in_schema=False)
def read_root():
    return RedirectResponse(url="/docs")

@app.on_event("startup")
def on_startup():
    create_db_and_tables()
    if not (os.getenv("LITELLM_MASTER_KEY") or "").strip():
        logging.warning(
            "LITELLM_MASTER_KEY is not set; planner and task LLM calls will fail until it is set "
            "(same value as llm-orchestrator). See .env.example."
        )

class StartRunRequest(BaseModel):
    goal: str

class ApproveRunRequest(BaseModel):
    dag_json: str # The edited or approved JSON DAG

@app.post("/runs")
async def start_run(req: StartRunRequest, background_tasks: BackgroundTasks, session: Session = Depends(get_session)):
    run = Run(goal=req.goal)
    session.add(run)
    session.commit()
    session.refresh(run)
    
    executor = DAGExecutor(run.id)
    background_tasks.add_task(executor.plan, req.goal)
    
    return {"run_id": run.id, "status": run.status}

@app.get("/runs/{run_id}")
async def get_run_status(run_id: int, session: Session = Depends(get_session)):
    run = session.get(Run, run_id)
    if not run:
        raise HTTPException(status_code=404, detail="Run not found")
        
    tasks = session.exec(select(TaskNode).where(TaskNode.run_id == run_id)).all()
    
    return {
        "run": run,
        "tasks": tasks
    }

@app.post("/runs/{run_id}/approve")
async def approve_run(run_id: int, req: ApproveRunRequest, background_tasks: BackgroundTasks, session: Session = Depends(get_session)):
    run = session.get(Run, run_id)
    if not run:
        raise HTTPException(status_code=404, detail="Run not found")
    if run.status != "approval_required":
        raise HTTPException(status_code=400, detail="Run is not awaiting approval")

    # Wipe old subagents to replace with the edited DAG definition
    old_tasks = session.exec(select(TaskNode).where(TaskNode.run_id == run_id)).all()
    for ot in old_tasks:
        session.delete(ot)
        
    try:
        dag = json.loads(req.dag_json)
        for agent in dag.get("subagents", []):
            task = TaskNode(
                run_id=run_id,
                client_uuid=agent["client_uuid"],
                role=agent["role"],
                system_prompt=agent["system_prompt"],
                instructions=agent["instructions"],
            )
            task.dependencies = agent.get("dependencies", [])
            session.add(task)
            
        run.dag_json = req.dag_json
        run.status = "approved"
        session.commit()
        
    except json.JSONDecodeError:
        raise HTTPException(status_code=400, detail="Invalid JSON format for approved DAG")

    executor = DAGExecutor(run_id)
    background_tasks.add_task(executor.execute_dag)
    
    return {"status": "Execution started"}

@app.get("/runs/{run_id}/stream")
async def stream_run(run_id: int, session: Session = Depends(get_session)):
    """
    Streams updates to the React UI via SSE (Server-Sent Events) for real-time observability.
    Reads append-only events from the database ensuring "State Lives on Disk" guarantee.
    """
    async def event_generator():
        last_log_id = 0
        while True:
            # Poll DB for new logs
            with Session(session.bind) as db:
                logs = db.exec(
                    select(EventLog)
                    .where(EventLog.run_id == run_id)
                    .where(EventLog.id > last_log_id)
                    .order_by(EventLog.id.asc())
                ).all()
                
                for log in logs:
                    last_log_id = log.id
                    data = {
                        "task_id": log.task_id,
                        "type": log.event_type,
                        "content": log.content,
                        "timestamp": log.created_at.isoformat()
                    }
                    yield {"data": json.dumps(data)}
                
                run = db.get(Run, run_id)
                # Terminate stream cleanly when run is no longer actively executing
                if run and run.status in ["completed", "failed", "approval_required"]:
                    if len(logs) == 0:
                        yield {"data": json.dumps({"type": "close", "content": run.status})}
                        break
                        
            await asyncio.sleep(1.0)
            
    return EventSourceResponse(event_generator())
