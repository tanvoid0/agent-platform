# Image service

Standalone text-to-image service (FLUX via `diffusers`) exposing the OpenAI
`/v1/images/generations` shape. The agent-platform proxy routes the
`image_generation` capability here when `IMAGE_API_BASE` points at it.

Kept out of the main app on purpose: diffusion needs a heavy GPU stack (torch +
CUDA), and isolating it keeps the platform an orchestrator. The platform never
imports these deps — it only speaks HTTP to this service.

## Why a separate service (not Ollama)

Ollama is built on llama.cpp and has **no diffusion pipeline** — it cannot run
FLUX/SD. The `x/*-klein` MLX builds are Apple-only and fail on Windows. So image
generation runs here, on CUDA, and the platform brokers it by capability.

## Setup (RTX 5080 / Blackwell, Windows or Linux)

Blackwell (sm_120) needs a **CUDA cu128+** torch build. Install torch FIRST,
then the rest:

```bash
cd services/image-service
python -m venv .venv && . .venv/Scripts/activate   # Linux: . .venv/bin/activate

# Blackwell-capable torch (cu128 nightly at time of writing):
pip install --pre torch torchvision --index-url https://download.pytorch.org/whl/nightly/cu128

pip install -r requirements.txt
```

FLUX weights download from Hugging Face on first request (accept the model
license on the hub; `flux.1-dev` is gated, `flux.1-schnell` is open). Set
`HF_TOKEN` in the environment if a model is gated.

## Run

```bash
uvicorn app:app --host 0.0.0.0 --port 9000
```

Verify the GPU is seen:

```bash
curl http://127.0.0.1:9000/health
# {"status":"ok","cuda":true,"device":"NVIDIA GeForce RTX 5080", ...}
```

## Wire into agent-platform

In the platform's `.env`:

```
IMAGE_API_BASE=http://127.0.0.1:9000
IMAGE_DEFAULT_MODEL=flux.1-schnell
```

Then the capability router lights up automatically:

```bash
curl -s localhost:18410/v1/capabilities -H "Authorization: Bearer $KEY" | jq .resolved.image_generation
# "image_local"

curl -s localhost:18410/v1/images/generations -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' \
  -d '{"prompt":"a red fox in snow","model":"flux.1-schnell"}' | jq '.data[0].b64_json' | head -c 40
```

## Request body

| field | default | notes |
|-------|---------|-------|
| `prompt` | required | text prompt |
| `model` | `flux.1-schnell` | must be a key in `_MODEL_REPOS` |
| `n` | 1 | clamped to `IMAGE_SERVICE_MAX_N` (default 4) |
| `size` | `1024x1024` | `WxH`, 256..2048, rounded to /16 |
| `steps` | 4 (schnell) / 28 (dev) | inference steps |
| `guidance_scale` | pipeline default | CFG |
| `seed` | random | reproducibility |

Add models by extending `_MODEL_REPOS` in `app.py` (e.g. a FLUX.2 klein repo id
once you have non-MLX weights).
