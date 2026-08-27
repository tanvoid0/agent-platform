"""Self-check for the parts of the pipeline that decide something.

No framework and no fixtures: `python test_model_ops.py` from `worker/`, and it
needs none of the GPU stack. What it covers is the logic a wrong answer from is
expensive and silent — a resume onto the wrong dataset, a PII gate that does not
fire, a step total that makes the progress bar lie.

    cd worker && PYTHONPATH=. python test_model_ops.py
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

from model_ops.pipeline import checkpoints, pii_scan
from model_ops.pipeline.train_lora import planned_steps


def test_planned_steps() -> None:
    # 100 examples, effective batch of 16, 3 epochs -> 7 steps an epoch.
    assert planned_steps(100, 4, 4, 3) == 21
    # Never zero: a dataset smaller than one batch is still one step an epoch,
    # and a zero total would divide by zero in the ETA.
    assert planned_steps(1, 4, 4, 1) == 1
    assert planned_steps(0, 4, 4, 3) == 3


def test_checkpoint_discovery(tmp: Path) -> None:
    output = tmp / "checkpoint"
    assert checkpoints.last_checkpoint(output) is None

    for step in (50, 400, 100):
        (output / f"checkpoint-{step}").mkdir(parents=True)
    (output / "not-a-checkpoint").mkdir()
    found = checkpoints.last_checkpoint(output)
    # Highest step, not alphabetical: "checkpoint-50" sorts after
    # "checkpoint-400" as a string, which is the bug this asserts against.
    assert found is not None and found.name == "checkpoint-400", found
    assert checkpoints.checkpoint_step(found) == 400


def test_resume_requires_a_matching_fingerprint(tmp: Path) -> None:
    adapter = tmp / "adapters" / "v1"
    output = adapter / "checkpoint"
    (output / "checkpoint-200").mkdir(parents=True)

    config = {"base_model": "b", "lora_rank": 16, "epochs": 3, "train_examples": 100}
    current = checkpoints.fingerprint(config, "sha-of-the-dataset")

    # A checkpoint with nothing beside it is not resumable: it could have come
    # from any run at all.
    resume, reason = checkpoints.resolve(adapter, output, current)
    assert resume is None and "no fingerprint" in reason, reason

    checkpoints.write_fingerprint(adapter, current)
    resume, reason = checkpoints.resolve(adapter, output, current)
    assert resume is not None and resume.name == "checkpoint-200", reason

    # The dataset changed under it. This is the case the whole module exists
    # for: resuming here trains on a schedule computed for the old data and
    # skips the new examples entirely.
    moved = checkpoints.fingerprint(config, "a-different-dataset")
    resume, reason = checkpoints.resolve(adapter, output, moved)
    assert resume is None and "dataset_sha256" in reason, reason

    # So is a hyperparameter change.
    retuned = checkpoints.fingerprint({**config, "lora_rank": 32}, "sha-of-the-dataset")
    resume, reason = checkpoints.resolve(adapter, output, retuned)
    assert resume is None and "lora_rank" in reason, reason

    # And the operator can always say no.
    resume, reason = checkpoints.resolve(adapter, output, current, enabled=False)
    assert resume is None and "disabled" in reason, reason


def test_dataset_digest_tracks_content(tmp: Path) -> None:
    path = tmp / "train.jsonl"
    path.write_text('{"messages":[]}\n', encoding="utf-8")
    first = checkpoints.dataset_digest(path)
    path.write_text('{"messages":[]}\n{"messages":[]}\n', encoding="utf-8")
    assert checkpoints.dataset_digest(path) != first


def rows(*texts: str) -> list[dict]:
    return [{"messages": [{"role": "user", "content": t}, {"role": "assistant", "content": "ok"}]} for t in texts]


def test_pii_scan_catches_what_it_must() -> None:
    for text, kind in [
        ("contact me at ben@example.co.uk please", "email"),
        ("call 07700 900123 after six", "phone"),
        ("+44 20 7946 0958 is the desk", "phone"),
        ("lives at SW1A 1AA in London", "postcode"),
        # A real prefix. "QQ" is the one HMRC prints in its own examples
        # precisely because it can never be issued, and the pattern excludes
        # the never-issued letters on purpose.
        ("NI number AB 12 34 56 C on file", "national_insurance"),
        ("card 4111111111111111 on record", "long_number"),
    ]:
        found = pii_scan.scan_rows(rows(text))
        assert found, f"missed {kind} in {text!r}"
        assert any(f.kind == kind for f in found), f"{text!r} -> {[f.kind for f in found]}"


def test_pii_scan_leaves_the_training_signal_alone() -> None:
    # Every one of these appears in a real job advert. A gate that fires on
    # them is a gate that gets turned off.
    clean = rows(
        "Salary £55,000 - £65,000 depending on experience",
        "We are hiring a backend engineer, London or remote (SW1 area)",
        "Founded 2019 - 2023 growth was 400%, team of 12",
        "Apply by 30/09/2026, ref 4821",
        "Stack: Python 3.11, Postgres 16, Kubernetes 1.29",
        "Contact {{EMAIL}} or {{PHONE}} — masked upstream",
    )
    found = pii_scan.scan_rows(clean)
    assert not found, [f"{f.kind}@row{f.line}" for f in found]


def test_require_clean_fails_loudly_and_says_nothing() -> None:
    try:
        pii_scan.require_clean(rows("mail ben@example.com"), "project x")
    except ValueError as e:
        message = str(e)
        assert "email" in message and "project x" in message
        # The scanner must not copy the thing it found into the log it is
        # protecting.
        assert "ben@example.com" not in message, message
    else:
        raise AssertionError("require_clean accepted a row with an email in it")


def main() -> int:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        checks = [
            (test_planned_steps, ()),
            (test_checkpoint_discovery, (tmp / "a",)),
            (test_resume_requires_a_matching_fingerprint, (tmp / "b",)),
            (test_dataset_digest_tracks_content, (tmp / "c",)),
            (test_pii_scan_catches_what_it_must, ()),
            (test_pii_scan_leaves_the_training_signal_alone, ()),
            (test_require_clean_fails_loudly_and_says_nothing, ()),
        ]
        failures = 0
        for check, args in checks:
            for arg in args:
                arg.mkdir(parents=True, exist_ok=True)
            try:
                check(*args)
                print(f"ok   {check.__name__}")
            except AssertionError as e:
                failures += 1
                print(f"FAIL {check.__name__}: {e}")
        print(f"\n{len(checks) - failures}/{len(checks)} passed")
        return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
