"""LoRA target module resolution — Gemma 4 multimodal compat."""

from __future__ import annotations

import re

_GEMMA4_LM_REGEX = (
    r".*language_model.*\.(q_proj|k_proj|v_proj|o_proj|gate_proj|up_proj|down_proj)"
)
_DEFAULT_TARGETS = [
    "q_proj",
    "k_proj",
    "v_proj",
    "o_proj",
    "gate_proj",
    "up_proj",
    "down_proj",
]
_MULTIMODAL_BLOCKLIST = ("vision_tower", "audio_tower")


def _supported_linear_types() -> tuple[type, ...]:
    import torch.nn as nn

    types: list[type] = [nn.Linear]
    try:
        from bitsandbytes.nn import Linear4bit

        types.append(Linear4bit)
    except ImportError:
        pass
    return tuple(types)


def is_gemma4_model(base_model: str) -> bool:
    mid = base_model.lower()
    return "gemma-4" in mid or "gemma4" in mid


def resolve_lora_targets(model, base_model: str) -> list[str] | str:
    if not is_gemma4_model(base_model):
        return _DEFAULT_TARGETS

    supported = _supported_linear_types()
    lm_layers = [
        name
        for name, mod in model.named_modules()
        if re.search(_GEMMA4_LM_REGEX, name) and isinstance(mod, supported)
    ]
    if lm_layers:
        return _GEMMA4_LM_REGEX

    explicit = [
        name
        for name, mod in model.named_modules()
        if isinstance(mod, supported)
        and any(t in name for t in _DEFAULT_TARGETS)
        and not any(b in name for b in _MULTIMODAL_BLOCKLIST)
    ]
    if explicit:
        return explicit

    raise RuntimeError(
        "Gemma 4 LoRA target resolution failed. "
        "Set fallback_base_model in project.yaml or upgrade peft>=0.19.0."
    )
