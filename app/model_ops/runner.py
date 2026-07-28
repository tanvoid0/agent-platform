"""Run model build pipeline stages (optionally in subprocess for GPU isolation)."""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any

from sqlmodel import Session

from model_ops.config_bridge import register_ollama_alias
from model_ops.models import ModelBuildJob, ModelProject
from model_ops.registry_hook import set_registry_callback
from model_ops.service import append_job_log, wire_registry_callback
from time_utils import utc_now_naive

_running: dict[int, asyncio.subprocess.Process] = {}

_REGISTRY_BOOTSTRAP = """
import os
if os.environ.get("MODEL_OPS_JOB_ID"):
    from model_ops.service import wire_registry_callback
    wire_registry_callback(None)
"""


def _use_subprocess_for_gpu() -> bool:
    return os.environ.get("MODEL_OPS_GPU_SUBPROCESS", "1").strip().lower() not in ("0", "false", "no")


def _app_pythonpath() -> str:
    return str(Path(__file__).resolve().parent.parent)


def _stage_script(stage: str, project: str, offline_eval: bool) -> list[str]:
    body = _REGISTRY_BOOTSTRAP + "\n"
    if stage == "prepare":
        body += (
            "from model_ops.pipeline.merge_knowledge import merge_packs\n"
            "from model_ops.pipeline.build_dataset import build_dataset\n"
            f"merge_packs({project!r})\n"
            f"build_dataset({project!r})\n"
        )
    elif stage == "train":
        body += f"from model_ops.pipeline.train_lora import train\ntrain({project!r})\n"
    elif stage == "export":
        body += f"from model_ops.pipeline.export_ollama import merge_and_export_gguf\nmerge_and_export_gguf({project!r})\n"
    elif stage == "eval":
        body += f"from model_ops.pipeline.eval import run_eval\nrun_eval({project!r}, offline={offline_eval!r})\n"
    else:
        raise ValueError(f"Unknown stage: {stage}")
    return [sys.executable, "-c", body]


async def _run_subprocess_stage(cmd: list[str], job: ModelBuildJob) -> int:
    env = {**os.environ, "PYTHONPATH": _app_pythonpath()}
    if job.id is not None:
        env["MODEL_OPS_JOB_ID"] = str(job.id)
    proc = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
        env=env,
    )
    assert job.id is not None
    _running[job.id] = proc
    assert proc.stdout is not None
    async for chunk in proc.stdout:
        append_job_log(job, chunk.decode(errors="replace"))
    code = await proc.wait()
    _running.pop(job.id, None)
    return code


async def _ollama_create_cb(tag: str, modelfile: str) -> tuple[bool, str]:
    from model_ops import ollama_client

    try:
        last = await ollama_client.create_model_sync(tag, modelfile)
        ok = last.get("status") == "success"
        return ok, str(last)
    except Exception as exc:
        return False, str(exc)


def _sync_ollama_create(tag: str, modelfile: str) -> tuple[bool, str]:
    return asyncio.run(_ollama_create_cb(tag, modelfile))


async def _run_stage(stage: str, project_name: str, job: ModelBuildJob, offline_eval: bool) -> dict[str, Any] | None:
    gpu_stages = {"train", "export"}
    if stage in gpu_stages and _use_subprocess_for_gpu():
        code = await _run_subprocess_stage(_stage_script(stage, project_name, offline_eval), job)
        if code != 0:
            raise RuntimeError(f"Stage {stage} exited with code {code}")
        return None

    if stage == "prepare":
        from model_ops.pipeline import build_dataset, merge_knowledge

        merge_knowledge.merge_packs(project_name)
        build_dataset.build_dataset(project_name)
    elif stage == "train":
        from model_ops.pipeline import train_lora

        train_lora.train(project_name)
    elif stage == "export":
        from model_ops.pipeline import export_ollama

        export_ollama.merge_and_export_gguf(project_name, ollama_create_fn=_sync_ollama_create)
    elif stage == "eval":
        from model_ops.pipeline import eval as eval_mod

        return eval_mod.run_eval(project_name, offline=offline_eval)
    else:
        raise ValueError(f"Unknown stage: {stage}")
    return None


async def run_ollama_job(session: Session, job: ModelBuildJob) -> None:
    from model_ops import ollama_client

    op = json.loads(job.operation_json or "{}")
    job.status = "running"
    job.started_at = utc_now_naive()
    job.current_stage = job.job_type
    session.add(job)
    session.commit()

    try:
        if job.job_type == "ollama_pull":
            name = str(op.get("name", ""))
            append_job_log(job, f"Pulling {name}...\n")
            last = await ollama_client.pull_model_sync(name)
            job.set_result({"ollama": last})
        elif job.job_type == "ollama_copy":
            source = str(op.get("source", ""))
            dest = str(op.get("destination", ""))
            append_job_log(job, f"Copying {source} -> {dest}...\n")
            last = await ollama_client.copy_model_sync(source, dest)
            job.set_result({"ollama": last})
        else:
            raise ValueError(f"Unknown ollama job type: {job.job_type}")

        job.status = "succeeded"
        job.finished_at = utc_now_naive()
        session.add(job)
        session.commit()
    except Exception as e:
        job.status = "failed"
        job.error_message = str(e)[:2000]
        job.finished_at = utc_now_naive()
        append_job_log(job, f"ERROR: {e}\n")
        session.add(job)
        session.commit()


async def run_job(
    session: Session,
    job: ModelBuildJob,
    project_name: str | None = None,
    offline_eval: bool = False,
) -> None:
    if job.job_type != "pipeline":
        await run_ollama_job(session, job)
        return

    if not project_name:
        raise ValueError("project_name required for pipeline jobs")

    wire_registry_callback(session)
    job.status = "running"
    job.started_at = utc_now_naive()
    session.add(job)
    session.commit()

    try:
        for stage in job.get_stages():
            job.current_stage = stage
            session.add(job)
            session.commit()
            append_job_log(job, f"=== stage: {stage} ===\n")
            extra = await _run_stage(stage, project_name, job, offline_eval)
            if extra is not None:
                job.set_result({**job.get_result(), "eval": extra})
                session.add(job)
                session.commit()

        if job.register_alias:
            project = session.get(ModelProject, job.project_id)
            if project:
                manifest = project.get_manifest()
                tag = manifest.get("ollama_tag", project.name)
                register_ollama_alias(job.register_alias, str(tag))

        job.status = "succeeded"
        job.finished_at = utc_now_naive()
        session.add(job)
        session.commit()
    except Exception as e:
        job.status = "failed"
        job.error_message = str(e)[:2000]
        job.finished_at = utc_now_naive()
        append_job_log(job, f"ERROR: {e}\n")
        session.add(job)
        session.commit()
    finally:
        set_registry_callback(None)


def cancel_job(job_id: int) -> bool:
    proc = _running.get(job_id)
    if proc is None:
        return False
    proc.terminate()
    _running.pop(job_id, None)
    return True
