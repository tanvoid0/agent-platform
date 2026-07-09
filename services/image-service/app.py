"""Local image-generation service (OpenAI /v1/images/generations shape).

Standalone FastAPI service the agent-platform proxy points at via IMAGE_API_BASE.
Runs FLUX (or any diffusers text-to-image pipeline) on a local CUDA GPU so the
platform stays an orchestrator and the heavy diffusion work is isolated here.

Contract (subset of the OpenAI images API the proxy forwards):
    POST /v1/images/generations
        { "prompt": str, "model": str?, "n": int?, "size": "WxH"?,
          "steps": int?, "guidance_scale": float?, "seed": int?,
          "response_format": "b64_json" }
    -> { "created": int, "data": [ { "b64_json": str }, ... ] }

The model is loaded lazily on first request and cached, so startup is cheap and
the GPU is only touched when an image is actually requested.
"""

from __future__ import annotations

import base64
import io
import os
import time
from threading import Lock
from typing import Any

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

app = FastAPI(title="agent-platform image service")

# Map an OpenAI-style model id -> diffusers repo. Extend as you pull weights.
_MODEL_REPOS: dict[str, str] = {
    "flux.1-schnell": "black-forest-labs/FLUX.1-schnell",
    "flux.1-dev": "black-forest-labs/FLUX.1-dev",
}
_DEFAULT_MODEL = os.environ.get("IMAGE_SERVICE_DEFAULT_MODEL", "flux.1-schnell")
_MAX_N = int(os.environ.get("IMAGE_SERVICE_MAX_N", "4"))

_pipelines: dict[str, Any] = {}
_pipeline_lock = Lock()


class ImageRequest(BaseModel):
    prompt: str = Field(..., min_length=1)
    model: str | None = None
    n: int = 1
    size: str = "1024x1024"
    steps: int | None = None
    guidance_scale: float | None = None
    seed: int | None = None
    response_format: str = "b64_json"


def _parse_size(size: str) -> tuple[int, int]:
    try:
        w_str, h_str = size.lower().split("x", 1)
        w, h = int(w_str), int(h_str)
    except (ValueError, AttributeError) as e:
        raise HTTPException(status_code=400, detail=f"invalid size: {size!r} (want WxH)") from e
    # FLUX wants multiples of 16; clamp to a sane range.
    if not (256 <= w <= 2048 and 256 <= h <= 2048):
        raise HTTPException(status_code=400, detail="size must be within 256..2048 per side")
    return w - (w % 16), h - (h % 16)


def _load_pipeline(model_id: str) -> Any:
    """Lazy-load + cache a diffusers pipeline for the given model id."""
    if model_id in _pipelines:
        return _pipelines[model_id]
    with _pipeline_lock:
        if model_id in _pipelines:
            return _pipelines[model_id]
        repo = _MODEL_REPOS.get(model_id)
        if repo is None:
            raise HTTPException(
                status_code=404,
                detail=f"unknown model {model_id!r}; known: {sorted(_MODEL_REPOS)}",
            )
        try:
            import torch
            from diffusers import FluxPipeline
        except ImportError as e:  # pragma: no cover - depends on host GPU stack
            raise HTTPException(
                status_code=500,
                detail=f"image backend not installed: {e}. See services/image-service/README.md",
            ) from e

        dtype = torch.bfloat16 if torch.cuda.is_available() else torch.float32
        pipe = FluxPipeline.from_pretrained(repo, torch_dtype=dtype)
        if torch.cuda.is_available():
            pipe = pipe.to("cuda")
        else:  # keep it runnable on CPU boxes for smoke tests (slow)
            pipe.enable_attention_slicing()
        _pipelines[model_id] = pipe
        return pipe


def _default_steps(model_id: str, requested: int | None) -> int:
    if requested is not None:
        return max(1, min(requested, 100))
    # schnell is a few-step distilled model; dev wants more.
    return 4 if "schnell" in model_id else 28


@app.get("/health")
def health() -> dict[str, Any]:
    try:
        import torch

        cuda = torch.cuda.is_available()
        device = torch.cuda.get_device_name(0) if cuda else None
    except ImportError:
        cuda, device = False, None
    return {
        "status": "ok",
        "cuda": cuda,
        "device": device,
        "models": sorted(_MODEL_REPOS),
        "loaded": sorted(_pipelines),
    }


@app.get("/v1/models")
def list_models() -> dict[str, Any]:
    return {
        "object": "list",
        "data": [{"id": mid, "object": "model", "owned_by": "image_local"} for mid in _MODEL_REPOS],
    }


@app.post("/v1/images/generations")
def generate(req: ImageRequest) -> dict[str, Any]:
    if req.response_format != "b64_json":
        raise HTTPException(status_code=400, detail="only response_format=b64_json is supported")
    n = max(1, min(req.n, _MAX_N))
    model_id = (req.model or _DEFAULT_MODEL).strip() or _DEFAULT_MODEL
    width, height = _parse_size(req.size)
    pipe = _load_pipeline(model_id)

    try:
        import torch

        generator = None
        if req.seed is not None:
            device = "cuda" if torch.cuda.is_available() else "cpu"
            generator = torch.Generator(device=device).manual_seed(req.seed)

        kwargs: dict[str, Any] = {
            "prompt": [req.prompt] * n,
            "width": width,
            "height": height,
            "num_inference_steps": _default_steps(model_id, req.steps),
            "generator": generator,
        }
        if req.guidance_scale is not None:
            kwargs["guidance_scale"] = req.guidance_scale
        result = pipe(**kwargs)
    except HTTPException:
        raise
    except Exception as e:  # pragma: no cover - runtime GPU failures
        raise HTTPException(status_code=500, detail=f"generation failed: {e}") from e

    data: list[dict[str, str]] = []
    for image in result.images:
        buf = io.BytesIO()
        image.save(buf, format="PNG")
        data.append({"b64_json": base64.b64encode(buf.getvalue()).decode("ascii")})

    return {"created": int(time.time()), "data": data}
