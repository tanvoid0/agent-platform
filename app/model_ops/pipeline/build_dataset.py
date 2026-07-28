"""Build train.jsonl and eval.jsonl from project knowledge (generic chat JSONL)."""

from __future__ import annotations

import json
import random
from pathlib import Path

import jsonschema

from model_ops.pipeline.jsonl_utils import load_jsonl, write_jsonl
from model_ops.pipeline.project_loader import data_root, get_project_dir, load_input_schema, load_project


def _is_chat_example(row: dict) -> bool:
    messages = row.get("messages")
    return isinstance(messages, list) and len(messages) >= 2


def _collect_jsonl_examples(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [row for row in load_jsonl(path) if _is_chat_example(row)]


def _validate_example(example: dict, schema: dict) -> dict | None:
    try:
        user_content = example["messages"][0]["content"]
        user_obj = json.loads(user_content)
        jsonschema.validate(user_obj, schema)
        return example
    except (json.JSONDecodeError, jsonschema.ValidationError, KeyError, IndexError):
        return None


def build_dataset(project: str, eval_ratio: float = 0.1, seed: int = 42) -> tuple[Path, Path]:
    project_dir = get_project_dir(project)
    schema = load_input_schema(project)
    manifest = load_project(project)
    datasets_dir = project_dir / "datasets"
    datasets_dir.mkdir(parents=True, exist_ok=True)

    examples: list[dict] = []

    knowledge_dir = project_dir / "knowledge"
    if knowledge_dir.is_dir():
        for jsonl_path in sorted(knowledge_dir.rglob("*.jsonl")):
            if "_merged" in jsonl_path.parts:
                continue
            examples.extend(_collect_jsonl_examples(jsonl_path))

    for rel in manifest.get("shared_datasets", []):
        shared_path = project_dir / rel
        if not shared_path.exists():
            shared_path = data_root() / "projects" / rel.lstrip("/")
        if shared_path.exists():
            examples.extend(_collect_jsonl_examples(shared_path))

    for rel in manifest.get("external_datasets", []):
        ext_path = project_dir / rel
        if ext_path.exists():
            examples.extend(_collect_jsonl_examples(ext_path))

    prebuilt_train = project_dir / "datasets" / "source_train.jsonl"
    if prebuilt_train.exists():
        examples.extend(_collect_jsonl_examples(prebuilt_train))

    validated: list[dict] = []
    for ex in examples:
        ok = _validate_example(ex, schema)
        if ok is not None:
            validated.append(ok)
    examples = validated

    seen_users: set[str] = set()
    deduped: list[dict] = []
    for ex in examples:
        key = ex["messages"][0]["content"]
        if key not in seen_users:
            seen_users.add(key)
            deduped.append(ex)
    examples = deduped

    if not examples:
        raise ValueError(
            f"No training examples for project {project}. "
            "Add chat JSONL files under knowledge/ with messages[{role, content}]."
        )

    random.seed(seed)
    random.shuffle(examples)
    split = max(1, int(len(examples) * eval_ratio))
    eval_set = examples[:split]
    train_set = examples[split:]

    train_path = datasets_dir / "train.jsonl"
    eval_path = datasets_dir / "eval.jsonl"
    write_jsonl(train_path, train_set)
    write_jsonl(eval_path, eval_set)
    return train_path, eval_path
