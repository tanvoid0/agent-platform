"""Decide whether a half-finished run can be picked up where it stopped.

A LoRA fine-tune is the one stage measured in hours, so it is the one stage a
power cut, an OOM, or a cancelled job wastes. `SFTTrainer` already writes
checkpoints; what was missing is the decision to use one, and the check that
using it is safe.

The check is the whole point. `resume_from_checkpoint` restores optimizer state
and a step counter, and it does not verify that the data or the hyperparameters
behind them still match. Resuming a run whose dataset has since doubled gives a
model trained on a schedule computed for the old one, with a step counter that
skips the new examples: it produces a plausible adapter that quietly saw less
than it claims. So a checkpoint is only resumable when its **fingerprint** —
the run's configuration and the exact bytes of its dataset — matches the run
about to start. Anything else starts clean and says why.
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any

FINGERPRINT_FILE = "fingerprint.json"

# `checkpoint-400`, written by the HuggingFace trainer.
_CHECKPOINT_DIR = re.compile(r"^checkpoint-(\d+)$")

# The config keys a resume is only valid across if they are unchanged. Anything
# that moves the loss landscape or the step schedule belongs here; things that
# do not (the timestamp, the adapter version label) must not, or every run looks
# like a mismatch and nothing ever resumes.
FINGERPRINT_KEYS = (
    "base_model",
    "lora_rank",
    "lora_alpha",
    "epochs",
    "max_seq_len",
    "learning_rate",
    "batch_size",
    "gradient_accumulation_steps",
    "train_examples",
    "init_from",
)


def dataset_digest(path: Path) -> str:
    """SHA-256 of the training file, read in blocks so a large one is fine."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def fingerprint(config: dict[str, Any], dataset_sha: str) -> dict[str, Any]:
    return {
        "dataset_sha256": dataset_sha,
        **{key: config.get(key) for key in FINGERPRINT_KEYS},
    }


def last_checkpoint(output_dir: Path) -> Path | None:
    """The highest-numbered `checkpoint-N` under `output_dir`, if any.

    `transformers.trainer_utils.get_last_checkpoint` does this, but importing it
    drags transformers into a call path that runs before the GPU deps are
    needed, and the rule is four lines.
    """
    if not output_dir.is_dir():
        return None
    found: list[tuple[int, Path]] = []
    for child in output_dir.iterdir():
        match = _CHECKPOINT_DIR.match(child.name)
        if match and child.is_dir():
            found.append((int(match.group(1)), child))
    if not found:
        return None
    return max(found)[1]


def checkpoint_step(checkpoint: Path) -> int:
    match = _CHECKPOINT_DIR.match(checkpoint.name)
    return int(match.group(1)) if match else 0


def resolve(
    adapter_dir: Path,
    output_dir: Path,
    current: dict[str, Any],
    enabled: bool = True,
) -> tuple[Path | None, str]:
    """`(checkpoint_to_resume_from, reason)`.

    `reason` is written to the job log either way, because "started from
    scratch" and "resumed at step 400" are the two facts someone reading a
    three-hour log needs to see at the top of it, and a silent decision here is
    the kind that gets discovered by a confusing eval score a day later.
    """
    if not enabled:
        return None, "resume disabled for this run"

    checkpoint = last_checkpoint(output_dir)
    if checkpoint is None:
        return None, "no checkpoint on disk"

    stored_path = adapter_dir / FINGERPRINT_FILE
    if not stored_path.exists():
        return None, f"checkpoint at {checkpoint.name} has no fingerprint beside it"

    try:
        stored = json.loads(stored_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None, f"fingerprint beside {checkpoint.name} is unreadable"

    changed = [key for key, value in current.items() if stored.get(key) != value]
    if changed:
        return None, f"{checkpoint.name} was trained with different {', '.join(sorted(changed))}"

    return checkpoint, f"resuming from {checkpoint.name}"


def write_fingerprint(adapter_dir: Path, current: dict[str, Any]) -> None:
    adapter_dir.mkdir(parents=True, exist_ok=True)
    (adapter_dir / FINGERPRINT_FILE).write_text(
        json.dumps(current, indent=2, sort_keys=True), encoding="utf-8"
    )
