"""Incremental LoRA training from approved feedback examples.

A second round of training is not a second training run. Two things have to be
true or the adapter forgets what the first round taught it:

- it starts from the **previous adapter**, not from the base model's fresh
  zero-initialised one (`init_from`), and
- its dataset mixes the new examples with a **replay sample** of the old ones,
  because a few hundred corrections on their own will happily overwrite
  everything the first round learned.

Both were parameters this module accepted and then ignored. `from_adapter` was
never passed on, and the round's dataset was copied over `datasets/train.jsonl`
— which is the file the replay sample is drawn from, so each continuation
replayed a pool that the previous continuation had already replaced with its
own examples. The history got thinner every round.
"""

from __future__ import annotations

import random
from pathlib import Path

from model_ops import progress
from model_ops.pipeline.jsonl_utils import load_jsonl, write_jsonl
from model_ops.pipeline.project_loader import get_project_dir
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
    progress.note(
        progress.PHASE_LOAD,
        f"{len(new_examples)} new examples, {len(replay)} replayed",
    )
    return out_path


def incremental_train(project: str, adapter_version: str = "v2", from_adapter: str = "v1") -> Path:
    inc_path = build_incremental_dataset(project)
    return train(
        project,
        adapter_version=adapter_version,
        init_from=from_adapter,
        dataset=inc_path,
    )
