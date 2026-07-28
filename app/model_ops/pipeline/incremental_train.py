"""Incremental LoRA training from approved feedback examples."""

from __future__ import annotations

import random
from pathlib import Path

from model_ops.pipeline.jsonl_utils import load_jsonl, write_jsonl
from model_ops.pipeline.project_loader import get_project_dir, load_project
from model_ops.pipeline.train_lora import train


def build_incremental_dataset(
    project: str,
    replay_ratio: float = 0.2,
    max_new: int = 500,
    seed: int = 42,
) -> Path:
    project_dir = get_project_dir(project)
    approved_path = project_dir / "datasets" / "feedback" / "approved.jsonl"
    base_train_path = project_dir / "datasets" / "train.jsonl"
    if not approved_path.exists():
        raise FileNotFoundError(f"No approved feedback data: {approved_path}")

    new_examples = load_jsonl(approved_path)[:max_new]
    replay: list[dict] = []
    if base_train_path.exists():
        all_train = load_jsonl(base_train_path)
        random.seed(seed)
        replay_count = int(len(new_examples) * replay_ratio / max(0.01, 1 - replay_ratio))
        replay_count = min(replay_count, len(all_train))
        replay = random.sample(all_train, replay_count) if replay_count else []

    combined = new_examples + replay
    random.shuffle(combined)
    out_path = project_dir / "datasets" / "incremental_train.jsonl"
    write_jsonl(out_path, combined)
    return out_path


def incremental_train(project: str, adapter_version: str = "v2", from_adapter: str = "v1") -> Path:
    build_incremental_dataset(project)
    project_dir = get_project_dir(project)
    manifest = load_project(project)
    inc_path = project_dir / "datasets" / "incremental_train.jsonl"
    train_path = project_dir / "datasets" / "train.jsonl"
    train_path.write_text(inc_path.read_text(encoding="utf-8"), encoding="utf-8")
    return train(project, adapter_version=adapter_version)
