"""LoRA fine-tune a base model for a project.

Three things this stage owes the caller beyond an adapter directory:

- **Progress.** A fine-tune is the one stage measured in hours. It reports
  phase, step, loss and ETA on marker lines (`model_ops.progress`) so a client
  can draw a bar instead of tailing a `tqdm` animation through a log file.
- **Resumability.** It checkpoints as it goes and picks the last one up on the
  next attempt, but only when the run's fingerprint still matches — see
  `checkpoints.resolve`.
- **Continuation.** `init_from` starts from a previously trained adapter rather
  than from the base model's zero-initialised one, which is what makes a
  second round of examples add to a model instead of replacing it.
"""

from __future__ import annotations

import json
import math
from datetime import datetime, timezone
from pathlib import Path

from model_ops import progress
from model_ops.pipeline import checkpoints
from model_ops.pipeline.jsonl_utils import load_jsonl
from model_ops.pipeline.lora_targets import resolve_lora_targets
from model_ops.pipeline.project_loader import get_project_dir, load_project, require_base_model
from model_ops.pipeline.train_utils import bitsandbytes_config, cuda_device_map, require_cuda, resolve_precision
from model_ops.registry_hook import register_model_entry


def format_chat(example: dict, tokenizer=None) -> str:
    messages = example.get("messages", [])
    if tokenizer is not None and hasattr(tokenizer, "apply_chat_template"):
        try:
            return tokenizer.apply_chat_template(
                messages, tokenize=False, add_generation_prompt=False
            )
        except Exception:
            pass
    parts = []
    for msg in messages:
        role = msg["role"]
        content = msg["content"]
        if role == "user":
            parts.append(f"<start_of_turn>user\n{content}<end_of_turn>")
        else:
            parts.append(f"<start_of_turn>model\n{content}<end_of_turn>")
    return "\n".join(parts) + "\n"


def planned_steps(example_count: int, batch_size: int, grad_accum: int, epochs: int) -> int:
    """What `max_steps` will come out as, computed before the trainer exists.

    Needed up front for two things: a checkpoint interval that produces about
    ten checkpoints whatever the dataset size (a fixed 50 would never fire on a
    200-example run and would fire constantly on a 200,000-example one), and a
    total for the progress bar during the minutes before step 1.
    """
    per_epoch = max(1, math.ceil(example_count / max(1, batch_size * grad_accum)))
    return per_epoch * max(1, epochs)


def _make_progress_callback(clock: progress.Clock, total_hint: int, resumed_step: int):
    """Build the trainer callback.

    Defined in a factory because `transformers` is imported inside `train` on
    purpose — the module has to import on a machine with no GPU stack, so that
    `incremental_train` and the tests can reach the rest of it.
    """
    from transformers import TrainerCallback

    class _Reporter(TrainerCallback):
        def _emit(self, state, phase: str, **extra) -> None:
            total = int(getattr(state, "max_steps", 0) or total_hint)
            step = int(getattr(state, "global_step", 0) or 0)
            progress.emit(
                phase=phase,
                step=step,
                total_steps=total,
                epoch=round(float(getattr(state, "epoch", 0.0) or 0.0), 3),
                elapsed_s=round(clock.elapsed(), 1),
                eta_s=(lambda e: round(e, 1) if e is not None else None)(clock.eta(step, total)),
                resumed_from=resumed_step or None,
                gpu=progress.gpu_memory(),
                **extra,
            )

        def on_train_begin(self, args, state, control, **kwargs):
            self._emit(state, progress.PHASE_TRAIN, message="training started")

        def on_log(self, args, state, control, logs=None, **kwargs):
            logs = logs or {}
            # The last `on_log` of a run carries summary keys (`train_runtime`,
            # `train_samples_per_second`) and no loss. Reporting it as a step
            # update would blank the loss the UI has been showing.
            if "loss" not in logs:
                return
            self._emit(
                state,
                progress.PHASE_TRAIN,
                loss=round(float(logs["loss"]), 5),
                learning_rate=logs.get("learning_rate"),
                grad_norm=(
                    round(float(logs["grad_norm"]), 5) if isinstance(logs.get("grad_norm"), (int, float)) else None
                ),
            )

        def on_save(self, args, state, control, **kwargs):
            self._emit(state, progress.PHASE_TRAIN, message="checkpoint written", checkpoint=True)

    return _Reporter()


def train(
    project: str,
    adapter_version: str = "v1",
    dry_run: bool = False,
    max_samples: int | None = None,
    epochs: int | None = None,
    resume: bool = True,
    init_from: str | None = None,
    dataset: str | Path | None = None,
) -> Path:
    manifest = load_project(project)
    project_dir = get_project_dir(project)
    train_cfg = manifest.get("train", {})

    # `dataset` overrides the default file rather than overwriting it. An
    # earlier version of `incremental_train` copied its round's examples over
    # `train.jsonl`, which destroyed the replay pool it reads from on the next
    # round — each continuation trained on a thinner history than the last.
    train_path = Path(dataset) if dataset else project_dir / "datasets" / "train.jsonl"
    if not train_path.is_absolute():
        train_path = project_dir / train_path
    if not train_path.exists():
        raise FileNotFoundError(f"Missing {train_path}. Run build_dataset first.")

    adapter_dir = project_dir / "adapters" / adapter_version
    adapter_dir.mkdir(parents=True, exist_ok=True)
    output_dir = adapter_dir / "checkpoint"

    progress.note(progress.PHASE_LOAD, f"reading {train_path.name}")

    base_model = require_base_model(manifest, project)
    fallback = manifest.get("fallback_base_model")
    examples = load_jsonl(train_path)
    if max_samples and max_samples < len(examples):
        examples = examples[:max_samples]

    batch_size = train_cfg.get("batch_size", 4)
    grad_accum = train_cfg.get("gradient_accumulation_steps", 4)
    resolved_epochs = epochs if epochs is not None else train_cfg.get("epochs", 3)

    config = {
        "project": project,
        "base_model": base_model,
        "adapter_version": adapter_version,
        "train_examples": len(examples),
        "train_dataset": str(train_path.relative_to(project_dir)) if train_path.is_relative_to(project_dir) else str(train_path),
        "lora_rank": train_cfg.get("lora_rank", 16),
        "lora_alpha": train_cfg.get("lora_alpha", 32),
        "epochs": resolved_epochs,
        "max_seq_len": train_cfg.get("max_seq_len", 2048),
        "learning_rate": train_cfg.get("learning_rate", 2e-4),
        "batch_size": batch_size,
        "gradient_accumulation_steps": grad_accum,
        "init_from": init_from,
        "trained_at": datetime.now(timezone.utc).isoformat(),
    }

    total_hint = planned_steps(len(examples), batch_size, grad_accum, resolved_epochs)
    dataset_sha = checkpoints.dataset_digest(train_path)
    current_fingerprint = checkpoints.fingerprint(config, dataset_sha)
    config["dataset_sha256"] = dataset_sha

    resume_from, reason = checkpoints.resolve(adapter_dir, output_dir, current_fingerprint, enabled=resume)
    resumed_step = checkpoints.checkpoint_step(resume_from) if resume_from else 0
    progress.note(
        progress.PHASE_LOAD,
        reason,
        step=resumed_step,
        total_steps=total_hint,
        resumed_from=resumed_step or None,
    )

    if dry_run:
        return adapter_dir

    from datasets import Dataset
    from peft import LoraConfig, PeftModel, get_peft_model, prepare_model_for_kbit_training
    from transformers import AutoModelForCausalLM, AutoTokenizer
    from trl import SFTConfig, SFTTrainer

    require_cuda()
    progress.note(progress.PHASE_LOAD, f"loading base model {base_model}")

    def load_model_and_tokenizer(model_id: str):
        tok = AutoTokenizer.from_pretrained(model_id, trust_remote_code=True)
        if tok.pad_token is None:
            tok.pad_token = tok.eos_token
        mdl = AutoModelForCausalLM.from_pretrained(
            model_id,
            quantization_config=bitsandbytes_config(),
            device_map=cuda_device_map(),
            trust_remote_code=True,
        )
        return mdl, tok

    try:
        model, tokenizer = load_model_and_tokenizer(base_model)
    except Exception:
        if not fallback:
            raise
        progress.note(progress.PHASE_LOAD, f"base model unavailable, falling back to {fallback}")
        base_model = fallback
        config["base_model"] = base_model
        model, tokenizer = load_model_and_tokenizer(base_model)

    model = prepare_model_for_kbit_training(model)

    def attach_adapter(mdl, model_id: str):
        """Either continue a previous adapter or start a fresh one.

        `is_trainable=True` is what separates this from inference loading: a
        `PeftModel` loaded without it has its LoRA weights frozen, and training
        would run to completion, report a falling loss from the base model's
        own gradients, and save an adapter identical to the one it started
        with.
        """
        if init_from:
            prior = project_dir / "adapters" / init_from
            if not prior.is_dir():
                raise FileNotFoundError(f"No adapter to continue from: {prior}")
            progress.note(progress.PHASE_LOAD, f"continuing adapter {init_from}")
            return PeftModel.from_pretrained(mdl, str(prior), is_trainable=True)
        return get_peft_model(
            mdl,
            LoraConfig(
                r=config["lora_rank"],
                lora_alpha=config["lora_alpha"],
                target_modules=resolve_lora_targets(mdl, model_id),
                lora_dropout=0.05,
                bias="none",
                task_type="CAUSAL_LM",
            ),
        )

    try:
        model = attach_adapter(model, base_model)
    except Exception as e:
        if not fallback or "Gemma4ClippableLinear" not in str(e):
            raise
        base_model = fallback
        config["base_model"] = base_model
        model, tokenizer = load_model_and_tokenizer(base_model)
        model = prepare_model_for_kbit_training(model)
        model = attach_adapter(model, base_model)

    progress.note(progress.PHASE_LOAD, f"tokenizing {len(examples)} examples")
    texts = [format_chat(ex, tokenizer) for ex in examples]
    dataset_obj = Dataset.from_dict({"text": texts})

    # About ten checkpoints per run, whatever its length, and only the last two
    # kept: enough granularity that a crash costs minutes rather than hours,
    # without a disk full of 4-bit optimizer states.
    save_steps = max(10, min(200, total_hint // 10 or 10))

    sft_config = SFTConfig(
        output_dir=str(output_dir),
        num_train_epochs=config["epochs"],
        per_device_train_batch_size=batch_size,
        gradient_accumulation_steps=grad_accum,
        learning_rate=config["learning_rate"],
        max_length=config["max_seq_len"],
        logging_steps=10,
        save_strategy="steps",
        save_steps=save_steps,
        save_total_limit=2,
        report_to="none",
        dataset_text_field="text",
        **resolve_precision(),
    )

    # Written before the run, not after: a fingerprint that only appears on
    # success is worthless to the crash it exists for.
    checkpoints.write_fingerprint(adapter_dir, current_fingerprint)

    clock = progress.Clock(start_step=resumed_step)
    trainer = SFTTrainer(
        model=model,
        args=sft_config,
        train_dataset=dataset_obj,
        processing_class=tokenizer,
        callbacks=[_make_progress_callback(clock, total_hint, resumed_step)],
    )
    result = trainer.train(resume_from_checkpoint=str(resume_from) if resume_from else None)

    progress.note(progress.PHASE_SAVE, "saving adapter")
    model.save_pretrained(str(adapter_dir))
    tokenizer.save_pretrained(str(adapter_dir))

    config["steps"] = int(getattr(trainer.state, "global_step", 0) or 0)
    config["resumed_from_step"] = resumed_step or None
    config["train_loss"] = (
        round(float(result.training_loss), 5)
        if result is not None and getattr(result, "training_loss", None) is not None
        else None
    )
    (adapter_dir / "train_config.json").write_text(json.dumps(config, indent=2), encoding="utf-8")

    register_model_entry(
        {
            "ollama_tag": manifest.get("ollama_tag", project),
            "project": project,
            "version": adapter_version,
            "base_model": base_model,
            "adapter": f"projects/{project}/adapters/{adapter_version}",
            "created_at": config["trained_at"][:10],
            "train_examples": len(examples),
            # Lineage, per ADR 0015 §2.4 L6: which dataset produced these
            # weights, and which adapter they continue. A dataset later found
            # to carry something it should not can be traced to exactly the
            # models built from it.
            "dataset_sha256": dataset_sha,
            "init_from": init_from,
            "steps": config["steps"],
            "train_loss": config["train_loss"],
            "eval_score": None,
        },
        set_active=True,
    )
    progress.emit(
        phase=progress.PHASE_DONE,
        step=config["steps"],
        total_steps=config["steps"],
        elapsed_s=round(clock.elapsed(), 1),
        loss=config["train_loss"],
        message=f"adapter {adapter_version} saved",
    )
    return adapter_dir
