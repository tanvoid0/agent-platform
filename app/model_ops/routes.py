"""Model build/train REST routes."""

from __future__ import annotations

import asyncio
import json
from typing import Any

from fastapi import APIRouter, BackgroundTasks, Depends, File, Form, HTTPException, UploadFile
from fastapi.responses import StreamingResponse
from sqlmodel import Session

from api_tokens.auth import TokenPrincipal, require_scope, require_valid_token
from database import get_session
from model_ops import ollama_client
from model_ops.runner import cancel_job, run_job
from model_ops.schemas import (
    ModelBuildJobCreateRequest,
    ModelBuildJobOut,
    ModelBuildOperationOut,
    ModelBuildOperationRequest,
    ModelProjectCreateRequest,
    ModelProjectOut,
    ModelProjectsListOut,
    ModelRegistryEntryOut,
    ModelRegistryListOut,
    OllamaCopyRequest,
    OllamaCreateRequest,
    OllamaJobCreateRequest,
    OllamaModelShowOut,
    OllamaModelsOut,
    OllamaOperationOut,
    OllamaPullRequest,
)
from model_ops.service import (
    activate_registry_entry,
    create_build_job,
    create_ollama_job,
    create_project,
    get_job,
    get_project_by_name,
    job_to_out,
    list_projects,
    list_registry,
    save_knowledge_files,
    save_project_files,
)

router = APIRouter(prefix="/model-ops", tags=["model-ops"])


def _run_job_background(job_id: int, project_name: str | None, offline_eval: bool) -> None:
    from database import engine
    from sqlmodel import Session as SqlSession

    with SqlSession(engine) as session:
        job = get_job(session, job_id)
        if job is None:
            return
        asyncio.run(run_job(session, job, project_name, offline_eval=offline_eval))


def _enqueue_ollama_job(
    session: Session,
    background_tasks: BackgroundTasks,
    job_type: str,
    operation: dict[str, Any],
) -> ModelBuildJobOut:
    job = create_ollama_job(session, job_type, operation)
    assert job.id is not None
    background_tasks.add_task(_run_job_background, job.id, None, False)
    return job_to_out(session, job)


@router.get(
    "/ollama/models",
    response_model=OllamaModelsOut,
    summary="List Ollama models",
    description="Proxy to Ollama GET /api/tags. Requires scope `model:read`.",
)
async def ollama_list_models(
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:read")
    try:
        payload = await ollama_client.list_models()
    except RuntimeError as e:
        raise HTTPException(status_code=503, detail=str(e)) from e
    return OllamaModelsOut(models=payload.get("models") or [])


@router.get(
    "/ollama/models/{name:path}",
    response_model=OllamaModelShowOut,
    summary="Show Ollama model details",
)
async def ollama_show_model(
    name: str,
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:read")
    try:
        raw = await ollama_client.show_model(name)
    except RuntimeError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e
    return OllamaModelShowOut(
        name=raw.get("model") or name,
        modelfile=raw.get("modelfile"),
        parameters=raw.get("parameters"),
        details=raw.get("details"),
        raw=raw,
    )


@router.post(
    "/ollama/models/pull",
    response_model=OllamaOperationOut | ModelBuildJobOut,
    summary="Pull an Ollama model",
    description="Sync by default. Pass `async: true` to enqueue a tracked job (poll via `/jobs/{id}`).",
)
async def ollama_pull(
    body: OllamaPullRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    if body.async_job:
        return _enqueue_ollama_job(session, background_tasks, "ollama_pull", {"name": body.name})

    events: list[dict[str, Any]] = []
    try:
        async for ev in ollama_client.pull_model(body.name):
            events.append(ev)
    except RuntimeError as e:
        raise HTTPException(status_code=503, detail=str(e)) from e
    ok = not events or events[-1].get("status") in ("success", "pulling")
    return OllamaOperationOut(ok=ok, message=f"Pulled {body.name}", events=events[-20:])


@router.post(
    "/ollama/models/copy",
    response_model=OllamaOperationOut | ModelBuildJobOut,
    summary="Copy an Ollama model tag",
    description="Async by default (`async: true`). Set `async: false` for a blocking copy.",
)
async def ollama_copy(
    body: OllamaCopyRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    if body.async_job:
        return _enqueue_ollama_job(
            session,
            background_tasks,
            "ollama_copy",
            {"source": body.source, "destination": body.destination},
        )

    events: list[dict[str, Any]] = []
    try:
        async for ev in ollama_client.copy_model(body.source, body.destination):
            events.append(ev)
    except RuntimeError as e:
        raise HTTPException(status_code=503, detail=str(e)) from e
    ok = not events or events[-1].get("status") == "success"
    return OllamaOperationOut(
        ok=ok,
        message=f"Copied {body.source} -> {body.destination}",
        events=events[-20:],
    )


@router.post(
    "/ollama/jobs",
    response_model=ModelBuildJobOut,
    summary="Enqueue an Ollama pull or copy job",
)
def ollama_jobs_create(
    body: OllamaJobCreateRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    if body.operation == "pull":
        if not body.name:
            raise HTTPException(status_code=400, detail="name is required for pull")
        return _enqueue_ollama_job(session, background_tasks, "ollama_pull", {"name": body.name})
    if body.operation == "copy":
        if not body.source or not body.destination:
            raise HTTPException(status_code=400, detail="source and destination are required for copy")
        return _enqueue_ollama_job(
            session,
            background_tasks,
            "ollama_copy",
            {"source": body.source, "destination": body.destination},
        )
    raise HTTPException(status_code=400, detail="operation must be pull or copy")


@router.post(
    "/ollama/models/create",
    response_model=OllamaOperationOut,
    summary="Create an Ollama model from Modelfile",
)
async def ollama_create(
    body: OllamaCreateRequest,
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    events: list[dict[str, Any]] = []
    try:
        if body.modelfile:
            async for ev in ollama_client.create_model(body.name, modelfile=body.modelfile):
                events.append(ev)
        else:
            fields: dict[str, Any] = {}
            if body.from_model:
                fields["from"] = body.from_model
            if body.system:
                fields["system"] = body.system
            if body.quantize:
                fields["quantize"] = body.quantize
            async for ev in ollama_client.create_model(body.name, **fields):
                events.append(ev)
    except RuntimeError as e:
        raise HTTPException(status_code=503, detail=str(e)) from e
    ok = bool(events) and events[-1].get("status") == "success"
    return OllamaOperationOut(
        ok=ok,
        message=f"Created {body.name}" if ok else f"Create finished with status {events[-1].get('status') if events else 'unknown'}",
        events=events[-20:],
    )


@router.delete(
    "/ollama/models/{name:path}",
    response_model=OllamaOperationOut,
    summary="Delete an Ollama model",
)
async def ollama_delete(
    name: str,
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    try:
        await ollama_client.delete_model(name)
    except RuntimeError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e
    return OllamaOperationOut(ok=True, message=f"Deleted {name}")


# --- Projects ---


@router.get(
    "/projects",
    response_model=ModelProjectsListOut,
    summary="List model training projects",
)
def model_projects_list(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:read")
    return ModelProjectsListOut(projects=list_projects(session))


@router.post(
    "/projects",
    response_model=ModelProjectOut,
    summary="Create a model training project from template",
)
def model_projects_create(
    body: ModelProjectCreateRequest,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    try:
        return create_project(
            session,
            body.name,
            description=body.description,
            base_model=body.base_model,
            ollama_tag=body.ollama_tag,
        )
    except ValueError as e:
        raise HTTPException(status_code=409, detail=str(e)) from e
    except RuntimeError as e:
        raise HTTPException(status_code=500, detail=str(e)) from e


@router.get(
    "/projects/{name}",
    response_model=ModelProjectOut,
    summary="Get project manifest and registry entries",
)
def model_projects_get(
    name: str,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:read")
    try:
        return get_project_by_name(session, name)
    except FileNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e


@router.post(
    "/projects/{name}/knowledge",
    summary="Upload knowledge pack files",
    description="Multipart upload of files into projects/{name}/knowledge/{pack_name}/",
)
async def model_projects_upload_knowledge(
    name: str,
    pack_name: str = Form(default="uploads"),
    files: list[UploadFile] = File(...),
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    try:
        get_project_by_name(session, name)
    except FileNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e

    payloads: list[tuple[str, bytes]] = []
    for f in files:
        payloads.append((f.filename or "upload.bin", await f.read()))
    count = save_knowledge_files(name, pack_name, payloads)
    return {"uploaded": count, "pack": pack_name}


@router.post(
    "/projects/{name}/files",
    summary="Upload project workspace files",
    description=(
        "Multipart upload into the project directory. Use the `path` form field on each part "
        "(or filename) as the relative path, e.g. `datasets/train.jsonl`, `project.yaml`, "
        "`schemas/game_state.schema.json`."
    ),
)
async def model_projects_upload_files(
    name: str,
    files: list[UploadFile] = File(...),
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    try:
        get_project_by_name(session, name)
    except FileNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e

    payloads: list[tuple[str, bytes]] = []
    for f in files:
        rel = f.headers.get("path") if hasattr(f, "headers") else None
        rel = rel or f.filename or "upload.bin"
        payloads.append((rel, await f.read()))
    try:
        count = save_project_files(name, payloads)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return {"uploaded": count}


# --- Jobs ---


@router.post(
    "/jobs",
    response_model=ModelBuildJobOut,
    summary="Start a model build job",
)
def model_jobs_create(
    body: ModelBuildJobCreateRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    try:
        get_project_by_name(session, body.project)
    except FileNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e

    job = create_build_job(
        session,
        body.project,
        list(body.stages),
        register_alias=body.register_alias,
        process_id=body.process_id,
    )
    background_tasks.add_task(_run_job_background, job.id, body.project, body.offline_eval)
    assert job.id is not None
    return job_to_out(session, job)


@router.post(
    "/operations/build",
    response_model=ModelBuildOperationOut,
    summary="model.build orchestration operation",
    description="Start a model build job using the reusable operation contract.",
)
def model_build_operation(
    body: ModelBuildOperationRequest,
    background_tasks: BackgroundTasks,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    if body.operation != "model.build":
        raise HTTPException(status_code=400, detail="Unsupported operation; use model.build")
    out = model_jobs_create(body.input, background_tasks, session, principal)
    return ModelBuildOperationOut(
        job_id=out.id,
        poll_url=out.poll_url,
        stream_url=out.stream_url,
    )


@router.get(
    "/jobs/{job_id}",
    response_model=ModelBuildJobOut,
    summary="Get build job status",
)
def model_jobs_get(
    job_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:read")
    job = get_job(session, job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Job not found")
    return job_to_out(session, job)


@router.get(
    "/jobs/{job_id}/stream",
    summary="Stream build job logs (SSE)",
    description="Server-sent events with log lines. Content-Type: text/event-stream.",
)
async def model_jobs_stream(
    job_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:read")
    job = get_job(session, job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Job not found")

    async def _gen():
        from model_ops.service import read_job_log_tail

        last_len = 0
        while True:
            job_ref = get_job(session, job_id)
            if job_ref is None:
                break
            tail = read_job_log_tail(job_ref, lines=200)
            if len(tail) > last_len:
                chunk = tail[last_len:]
                last_len = len(tail)
                payload = json.dumps({"log": chunk, "status": job_ref.status, "stage": job_ref.current_stage})
                yield f"event: log\ndata: {payload}\n\n"
            if job_ref.status in ("succeeded", "failed", "cancelled"):
                done = json.dumps({"status": job_ref.status, "result": job_ref.get_result()})
                yield f"event: done\ndata: {done}\n\n"
                break
            await asyncio.sleep(1.0)

    return StreamingResponse(_gen(), media_type="text/event-stream")


@router.post(
    "/jobs/{job_id}/cancel",
    response_model=ModelBuildJobOut,
    summary="Cancel a running build job",
)
def model_jobs_cancel(
    job_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    job = get_job(session, job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Job not found")
    if job.status not in ("pending", "running"):
        raise HTTPException(status_code=409, detail=f"Job is {job.status}")
    cancel_job(job_id)
    job.status = "cancelled"
    from time_utils import utc_now_naive

    job.finished_at = utc_now_naive()
    session.add(job)
    session.commit()
    session.refresh(job)
    return job_to_out(session, job)


# --- Registry ---


@router.get(
    "/registry",
    response_model=ModelRegistryListOut,
    summary="List model registry entries",
)
def model_registry_list(
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:read")
    return ModelRegistryListOut(entries=list_registry(session))


@router.post(
    "/registry/{entry_id}/activate",
    response_model=ModelRegistryEntryOut,
    summary="Activate a registry entry for its project",
)
def model_registry_activate(
    entry_id: int,
    session: Session = Depends(get_session),
    principal: TokenPrincipal = Depends(require_valid_token),
):
    require_scope(principal, "model:write")
    try:
        return activate_registry_entry(session, entry_id)
    except ValueError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e
