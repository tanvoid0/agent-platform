"""Business logic for model-ops projects, jobs, and registry."""

from __future__ import annotations

import json
import shutil
from pathlib import Path
from typing import Any

import yaml
from sqlmodel import Session, select

from model_ops.models import ModelBuildJob, ModelProject, ModelRegistryEntry
from model_ops.paths import ensure_data_scaffold, projects_dir, template_project_dir
from model_ops.pipeline.project_loader import get_project_dir, load_project
from model_ops.registry_hook import register_model_entry, set_registry_callback
from model_ops.schemas import ModelBuildJobOut, ModelProjectOut, ModelRegistryEntryOut
from time_utils import utc_now_naive


def _sync_project_row(session: Session, name: str) -> ModelProject:
    ensure_data_scaffold()
    row = session.exec(select(ModelProject).where(ModelProject.name == name)).first()
    manifest = load_project(name)
    if row is None:
        row = ModelProject(name=name, description=manifest.get("description"))
        session.add(row)
    row.set_manifest(manifest)
    row.updated_at = utc_now_naive()
    session.commit()
    session.refresh(row)
    return row


def list_projects(session: Session) -> list[ModelProjectOut]:
    ensure_data_scaffold()
    rows = session.exec(select(ModelProject).order_by(ModelProject.name)).all()
    out: list[ModelProjectOut] = []
    for row in rows:
        try:
            load_project(row.name)
        except FileNotFoundError:
            continue
        out.append(project_to_out(session, row))
    return out


def registry_entries_for_project(session: Session, project_id: int) -> list[ModelRegistryEntryOut]:
    rows = session.exec(
        select(ModelRegistryEntry)
        .where(ModelRegistryEntry.project_id == project_id)
        .order_by(ModelRegistryEntry.created_at.desc())
    ).all()
    project = session.get(ModelProject, project_id)
    pname = project.name if project else None
    return [
        ModelRegistryEntryOut(
            id=r.id,
            project_id=r.project_id,
            project_name=pname,
            version=r.version,
            ollama_tag=r.ollama_tag,
            base_model=r.base_model,
            eval_score=r.eval_score,
            is_active=r.is_active,
        )
        for r in rows
        if r.id is not None
    ]


def project_to_out(session: Session, row: ModelProject) -> ModelProjectOut:
    assert row.id is not None
    return ModelProjectOut(
        id=row.id,
        name=row.name,
        description=row.description,
        manifest=row.get_manifest(),
        registry_entries=registry_entries_for_project(session, row.id),
    )


def create_project(
    session: Session,
    name: str,
    description: str | None = None,
    base_model: str | None = None,
    ollama_tag: str | None = None,
) -> ModelProjectOut:
    ensure_data_scaffold()
    dest = projects_dir() / name
    if dest.exists():
        raise ValueError(f"Project already exists: {name}")

    src = template_project_dir()
    if not src.is_dir():
        raise RuntimeError("Template project missing; check model_ops/data/projects/_template")

    shutil.copytree(src, dest)
    manifest_path = dest / "project.yaml"
    with manifest_path.open(encoding="utf-8") as f:
        data = yaml.safe_load(f) or {}
    data["name"] = name
    if description:
        data["description"] = description
    if base_model:
        data["base_model"] = base_model
    if ollama_tag:
        data["ollama_tag"] = ollama_tag
    manifest_path.write_text(yaml.safe_dump(data, sort_keys=False), encoding="utf-8")

    row = ModelProject(name=name, description=description or data.get("description"))
    row.set_manifest(data)
    session.add(row)
    session.commit()
    session.refresh(row)
    return project_to_out(session, row)


def get_project_by_name(session: Session, name: str) -> ModelProjectOut:
    row = _sync_project_row(session, name)
    return project_to_out(session, row)


def save_knowledge_files(project: str, pack_name: str, files: list[tuple[str, bytes]]) -> int:
    project_dir = get_project_dir(project)
    knowledge_dir = project_dir / "knowledge" / pack_name
    knowledge_dir.mkdir(parents=True, exist_ok=True)
    count = 0
    for rel, content in files:
        safe = Path(rel).name if "/" not in rel and "\\" not in rel else rel.replace("\\", "/")
        dest = knowledge_dir / safe
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(content)
        count += 1
    return count


def save_project_files(project: str, files: list[tuple[str, bytes]]) -> int:
    """Write files under the project workspace (e.g. project.yaml, datasets/train.jsonl)."""
    project_dir = get_project_dir(project)
    count = 0
    for rel, content in files:
        safe = rel.replace("\\", "/").lstrip("/")
        if not safe or safe.startswith("..") or "/.." in f"/{safe}/":
            raise ValueError(f"Invalid project path: {rel}")
        dest = project_dir / safe
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(content)
        count += 1
    return count


def persist_registry_entry(session: Session, entry: dict[str, Any], set_active: bool) -> None:
    project_name = entry.get("project") or entry.get("ollama_tag")
    if not project_name:
        return
    row = _sync_project_row(session, str(project_name))
    assert row.id is not None

    version = str(entry.get("version", "v1"))
    tag = str(entry.get("ollama_tag", project_name))

    existing = session.exec(
        select(ModelRegistryEntry).where(
            ModelRegistryEntry.project_id == row.id,
            ModelRegistryEntry.version == version,
        )
    ).first()

    if existing is None:
        existing = ModelRegistryEntry(
            project_id=row.id,
            version=version,
            ollama_tag=tag,
        )
        session.add(existing)

    existing.base_model = entry.get("base_model")
    existing.adapter_path = entry.get("adapter")
    existing.gguf_path = entry.get("gguf")
    if entry.get("eval_score") is not None:
        existing.eval_score = float(entry["eval_score"])
    existing.metadata_json = json.dumps(entry, ensure_ascii=False)

    if set_active:
        for other in session.exec(
            select(ModelRegistryEntry).where(ModelRegistryEntry.project_id == row.id)
        ).all():
            other.is_active = False
        existing.is_active = True
        existing.ollama_tag = tag

    session.commit()


def wire_registry_callback(session: Session | None = None) -> None:
    def _cb(entry: dict[str, Any], set_active: bool) -> None:
        if session is not None:
            persist_registry_entry(session, entry, set_active)
            return
        from database import engine
        from sqlmodel import Session as SqlSession

        with SqlSession(engine) as inner:
            persist_registry_entry(inner, entry, set_active)

    set_registry_callback(_cb)


def link_process_to_job(session: Session, job: ModelBuildJob) -> None:
    if not job.process_id or job.id is None:
        return
    from models import Process

    proc = session.get(Process, job.process_id)
    if proc is None:
        return
    proc.model_build_job_id = job.id
    session.add(proc)
    session.commit()


def create_ollama_job(session: Session, job_type: str, operation: dict[str, Any]) -> ModelBuildJob:
    logs_dir = ensure_data_scaffold() / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)
    job = ModelBuildJob(
        project_id=None,
        job_type=job_type,
        status="pending",
    )
    job.operation_json = json.dumps(operation, ensure_ascii=False)
    job.set_stages([])
    session.add(job)
    session.commit()
    session.refresh(job)
    assert job.id is not None
    log_path = logs_dir / f"job_{job.id}.log"
    job.log_path = str(log_path)
    log_path.write_text("", encoding="utf-8")
    session.add(job)
    session.commit()
    session.refresh(job)
    return job


def create_build_job(
    session: Session,
    project_name: str,
    stages: list[str],
    register_alias: str | None = None,
    process_id: int | None = None,
) -> ModelBuildJob:
    row = _sync_project_row(session, project_name)
    assert row.id is not None
    logs_dir = ensure_data_scaffold() / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)
    job = ModelBuildJob(
        project_id=row.id,
        job_type="pipeline",
        status="pending",
        register_alias=register_alias,
        process_id=process_id,
    )
    job.set_stages(stages)
    session.add(job)
    session.commit()
    session.refresh(job)
    assert job.id is not None
    log_path = logs_dir / f"job_{job.id}.log"
    job.log_path = str(log_path)
    log_path.write_text("", encoding="utf-8")
    session.add(job)
    session.commit()
    session.refresh(job)
    link_process_to_job(session, job)
    return job


def append_job_log(job: ModelBuildJob, text: str) -> None:
    if not job.log_path:
        return
    path = Path(job.log_path)
    with path.open("a", encoding="utf-8") as f:
        f.write(text)
        if not text.endswith("\n"):
            f.write("\n")


def read_job_log_tail(job: ModelBuildJob, lines: int = 80) -> str:
    if not job.log_path or not Path(job.log_path).exists():
        return ""
    content = Path(job.log_path).read_text(encoding="utf-8", errors="replace").splitlines()
    return "\n".join(content[-lines:])


def job_to_out(session: Session, job: ModelBuildJob, api_prefix: str = "/api/v1") -> ModelBuildJobOut:
    assert job.id is not None
    project_name: str | None = None
    if job.project_id is not None:
        project = session.get(ModelProject, job.project_id)
        project_name = project.name if project else "unknown"
    return ModelBuildJobOut(
        id=job.id,
        job_type=job.job_type,
        project_id=job.project_id,
        project_name=project_name,
        stages=job.get_stages(),
        status=job.status,  # type: ignore[arg-type]
        current_stage=job.current_stage,
        register_alias=job.register_alias,
        result=job.get_result(),
        error_message=job.error_message,
        log_tail=read_job_log_tail(job),
        poll_url=f"{api_prefix}/model-ops/jobs/{job.id}",
        stream_url=f"{api_prefix}/model-ops/jobs/{job.id}/stream",
        created_at=job.created_at.isoformat(),
        started_at=job.started_at.isoformat() if job.started_at else None,
        finished_at=job.finished_at.isoformat() if job.finished_at else None,
    )


def get_job(session: Session, job_id: int) -> ModelBuildJob | None:
    return session.get(ModelBuildJob, job_id)


def list_registry(session: Session) -> list[ModelRegistryEntryOut]:
    rows = session.exec(select(ModelRegistryEntry).order_by(ModelRegistryEntry.created_at.desc())).all()
    out: list[ModelRegistryEntryOut] = []
    for r in rows:
        if r.id is None:
            continue
        project = session.get(ModelProject, r.project_id)
        out.append(
            ModelRegistryEntryOut(
                id=r.id,
                project_id=r.project_id,
                project_name=project.name if project else None,
                version=r.version,
                ollama_tag=r.ollama_tag,
                base_model=r.base_model,
                eval_score=r.eval_score,
                is_active=r.is_active,
            )
        )
    return out


def activate_registry_entry(session: Session, entry_id: int) -> ModelRegistryEntryOut:
    row = session.get(ModelRegistryEntry, entry_id)
    if row is None:
        raise ValueError("Registry entry not found")
    for other in session.exec(
        select(ModelRegistryEntry).where(ModelRegistryEntry.project_id == row.project_id)
    ).all():
        other.is_active = False
    row.is_active = True
    session.add(row)
    session.commit()
    session.refresh(row)
    project = session.get(ModelProject, row.project_id)
    assert row.id is not None
    return ModelRegistryEntryOut(
        id=row.id,
        project_id=row.project_id,
        project_name=project.name if project else None,
        version=row.version,
        ollama_tag=row.ollama_tag,
        base_model=row.base_model,
        eval_score=row.eval_score,
        is_active=row.is_active,
    )
