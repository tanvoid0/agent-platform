"""Shared training helpers — device, precision, BitsAndBytes config."""

from __future__ import annotations


def require_cuda() -> None:
    import torch

    if torch.cuda.is_available():
        return

    raise RuntimeError(
        "CUDA GPU required for LoRA training, but torch.cuda.is_available() is False.\n"
        f"Installed torch: {torch.__version__}\n"
        "Install CUDA PyTorch from https://download.pytorch.org/whl/"
    )


def resolve_precision() -> dict[str, bool]:
    import torch

    if not torch.cuda.is_available():
        return {"bf16": False, "fp16": False}

    if torch.cuda.is_bf16_supported():
        return {"bf16": True, "fp16": False}

    return {"bf16": False, "fp16": True}


def bitsandbytes_config():
    import torch
    from transformers import BitsAndBytesConfig

    compute_dtype = (
        torch.bfloat16
        if torch.cuda.is_available() and torch.cuda.is_bf16_supported()
        else torch.float16
    )
    return BitsAndBytesConfig(
        load_in_4bit=True,
        bnb_4bit_quant_type="nf4",
        bnb_4bit_compute_dtype=compute_dtype,
        bnb_4bit_use_double_quant=True,
    )


def cuda_device_map() -> dict[str, int]:
    return {"": 0}
