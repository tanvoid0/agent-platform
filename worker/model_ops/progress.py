"""Report progress to the server that spawned this stage.

Same channel as ``registry_hook``: a marked line on stdout, which
``agent-platformd`` is already reading to tee into the job log. The parent
picks these out in ``model_ops.rs::handle_marker`` and keeps the newest one on
the job row, so a client that connects halfway through a two-hour fine-tune
still gets a number instead of a blank bar.

Why a marker rather than the trainer's own output: HuggingFace prints a `tqdm`
bar to stderr, which is a carriage-return animation. It is unparseable once it
has been through a log file, it carries no total when the run resumes, and it
says nothing about which phase the stage is in — a stage spends minutes loading
a quantized base model before step 1, and silence there looks like a hang.

The prefix is duplicated in ``model_ops.rs`` and must stay in step with it.
"""

from __future__ import annotations

import json
import sys
import time
from typing import Any

MARKER = "@@AGP:progress@@"

# Phases a stage moves through, coarse enough that a client can render them as a
# stepper without knowing anything about training.
PHASE_LOAD = "load"
PHASE_TRAIN = "train"
PHASE_SAVE = "save"
PHASE_DONE = "done"


def emit(**fields: Any) -> None:
    """Print one progress marker. Never raises: progress is not the job."""
    try:
        payload = json.dumps(fields, default=str)
    except (TypeError, ValueError):
        return
    # Flushed, and on stdout: the parent reads line by line and a stage killed
    # before its buffer drains would otherwise strand the bar at its last value.
    print(f"{MARKER} {payload}", flush=True)


def note(phase: str, message: str, **fields: Any) -> None:
    """A phase change with a human sentence attached.

    Used for the parts of a stage that have no step counter — loading a base
    model, merging an adapter, quantizing — where the only honest progress
    report is "still here, doing this".
    """
    emit(phase=phase, message=message, **fields)


class Clock:
    """Elapsed and ETA, with the resume offset accounted for.

    A resumed run starts at step 400 of 900 with zero seconds on the clock.
    Dividing elapsed by ``global_step`` there claims a rate four hundred steps
    faster than reality and shows an ETA of about a minute for half an hour of
    work. Steps are counted from where this process actually started.
    """

    def __init__(self, start_step: int = 0) -> None:
        self.started = time.monotonic()
        self.start_step = start_step

    def elapsed(self) -> float:
        return time.monotonic() - self.started

    def eta(self, step: int, total: int) -> float | None:
        done = step - self.start_step
        remaining = total - step
        if done <= 0 or remaining <= 0:
            return None
        return (self.elapsed() / done) * remaining


def gpu_memory() -> dict[str, int] | None:
    """Allocated and total VRAM in MB, or None when there is no CUDA device.

    Imported lazily and defensively — `prepare` and `eval` run in interpreters
    that have no torch at all, and this module is imported by both.
    """
    try:
        import torch

        if not torch.cuda.is_available():
            return None
        free, total = torch.cuda.mem_get_info()
        return {
            "allocated_mb": int(torch.cuda.memory_allocated() // (1024 * 1024)),
            "used_mb": int((total - free) // (1024 * 1024)),
            "total_mb": int(total // (1024 * 1024)),
        }
    except Exception:
        return None
