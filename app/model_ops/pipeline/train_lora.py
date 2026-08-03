"""LoRA fine-tune a base model for a project."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

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


def train(
    project: str,
    adapter_version: str = "v1",
    dry_run: bool = False,
    max_samples: int | None = None,
    epochs: int | None = None,
) -> Path:
    manifest = load_project(project)
    project_dir = get_project_dir(project)
    train_cfg = manifest.get("train", {})

    train_path = project_dir / "datasets" / "train.jsonl"
    if not train_path.exists():
        raise FileNotFoundError(f"Missing {train_path}. Run build_dataset first.")

    adapter_dir = project_dir / "adapters" / adapter_version
    adapter_dir.mkdir(parents=True, exist_ok=True)

    base_model = require_base_model(manifest, project)
    fallback = manifest.get("fallback_base_model")
    examples = load_jsonl(train_path)
    if max_samples and max_samples < len(examples):
        examples = examples[:max_samples]

    config = {
        "project": project,
        "base_model": base_model,
        "adapter_version": adapter_version,
        "train_examples": len(examples),
        "lora_rank": train_cfg.get("lora_rank", 16),
        "lora_alpha": train_cfg.get("lora_alpha", 32),
        "epochs": epochs if epochs is not None else train_cfg.get("epochs", 3),
        "max_seq_len": train_cfg.get("max_seq_len", 2048),
        "learning_rate": train_cfg.get("learning_rate", 2e-4),
        "batch_size": train_cfg.get("batch_size", 4),
        "gradient_accumulation_steps": train_cfg.get("gradient_accumulation_steps", 4),
        "trained_at": datetime.now(timezone.utc).isoformat(),
    }

    if dry_run:
        return adapter_dir

    from datasets import Dataset
    from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
    from transformers import AutoModelForCausalLM, AutoTokenizer
    from trl import SFTConfig, SFTTrainer

    require_cuda()

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
        base_model = fallback
        config["base_model"] = base_model
        model, tokenizer = load_model_and_tokenizer(base_model)

    model = prepare_model_for_kbit_training(model)

    target_modules = resolve_lora_targets(model, base_model)
    lora_config = LoraConfig(
        r=train_cfg.get("lora_rank", 16),
        lora_alpha=train_cfg.get("lora_alpha", 32),
        target_modules=target_modules,
        lora_dropout=0.05,
        bias="none",
        task_type="CAUSAL_LM",
    )
    try:
        model = get_peft_model(model, lora_config)
    except Exception as e:
        if not fallback or "Gemma4ClippableLinear" not in str(e):
            raise
        base_model = fallback
        config["base_model"] = base_model
        model, tokenizer = load_model_and_tokenizer(base_model)
        model = prepare_model_for_kbit_training(model)
        lora_config = LoraConfig(
            r=train_cfg.get("lora_rank", 16),
            lora_alpha=train_cfg.get("lora_alpha", 32),
            target_modules=resolve_lora_targets(model, base_model),
            lora_dropout=0.05,
            bias="none",
            task_type="CAUSAL_LM",
        )
        model = get_peft_model(model, lora_config)

    texts = [format_chat(ex, tokenizer) for ex in examples]
    dataset = Dataset.from_dict({"text": texts})

    output_dir = adapter_dir / "checkpoint"
    sft_config = SFTConfig(
        output_dir=str(output_dir),
        num_train_epochs=config["epochs"],
        per_device_train_batch_size=train_cfg.get("batch_size", 4),
        gradient_accumulation_steps=train_cfg.get("gradient_accumulation_steps", 4),
        learning_rate=train_cfg.get("learning_rate", 2e-4),
        max_length=train_cfg.get("max_seq_len", 2048),
        logging_steps=10,
        save_strategy="epoch",
        report_to="none",
        dataset_text_field="text",
        **resolve_precision(),
    )

    trainer = SFTTrainer(
        model=model,
        args=sft_config,
        train_dataset=dataset,
        processing_class=tokenizer,
    )
    trainer.train()
    model.save_pretrained(str(adapter_dir))
    tokenizer.save_pretrained(str(adapter_dir))
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
            "eval_score": None,
        },
        set_active=True,
    )
    return adapter_dir
