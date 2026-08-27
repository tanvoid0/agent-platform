"""Generate screening answers from a trained adapter, for the benchmark to score.

    PYTHONPATH=. python predict_screener.py <project> <holdout.jsonl> <out.jsonl> [adapter_version]

Writes one `{"id", "text", "out_tokens", "ms"}` per line — the same fields the
Ollama path reports — so `bench-screener.mjs --predictions` scores a local
adapter with the identical scorer it uses for a served model. One scorer, two
sources: a benchmark whose two halves can disagree about what "right" means is
not a benchmark.

This exists because a freshly trained adapter is not yet a GGUF and not yet in
Ollama, and waiting for the export to measure it means finding out an hour late
that the run was wasted.
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import torch
from peft import PeftModel
from transformers import AutoModelForCausalLM, AutoTokenizer

sys.path.insert(0, str(Path(__file__).parent))
from model_ops.pipeline.project_loader import get_project_dir, load_project  # noqa: E402


# Set MODEL_OPS_PREFILL=1 to open the JSON object for the model. Empty by
# default so a plain run measures the model alone.
PREFILL = '{"a":"' if os.environ.get("MODEL_OPS_PREFILL") == "1" else ""


def main() -> int:
    project, rows_path, out_path = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    version = sys.argv[4] if len(sys.argv) > 4 else "v1"

    manifest = load_project(project)
    adapter = get_project_dir(project) / "adapters" / version
    base = manifest["base_model"]

    tokenizer = AutoTokenizer.from_pretrained(base, trust_remote_code=True)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
    model = AutoModelForCausalLM.from_pretrained(
        base, dtype=torch.bfloat16, device_map={"": 0}, trust_remote_code=True
    )
    model = PeftModel.from_pretrained(model, str(adapter))
    model.eval()

    rows = [json.loads(line) for line in rows_path.read_text(encoding="utf-8").splitlines() if line.strip()]
    written = []

    for index, row in enumerate(rows):
        # The assistant turn is the answer. Prompting with it would measure
        # nothing but the tokenizer.
        messages = [m for m in row["messages"] if m["role"] != "assistant"]
        prompt = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
        # Prefill: start the answer for it. A small model that has learned the
        # reading but not the shape emits `{true,false,meets,"no"}` often enough
        # to lose a third of its replies; opening the object and the first key
        # removes that failure without touching the weights. This is the cheap
        # half of what a JSON grammar does at serving time, and it is measured
        # as its own run rather than folded into the model's score.
        prompt += PREFILL
        inputs = tokenizer(prompt, return_tensors="pt").to(model.device)

        started = time.monotonic()
        with torch.no_grad():
            # Greedy, and capped just past the answer's length: the benchmark
            # compares runs, so nothing here may vary between them, and a model
            # that has not closed the object in 64 tokens is not going to.
            output = model.generate(
                **inputs,
                max_new_tokens=64,
                do_sample=False,
                pad_token_id=tokenizer.pad_token_id,
            )
        elapsed_ms = int((time.monotonic() - started) * 1000)

        generated = output[0][inputs["input_ids"].shape[1]:]
        written.append({
            "id": row.get("id", f"row-{index}"),
            # The prefill is part of the answer, so it goes back on before the
            # scorer sees it; the token count deliberately does not include it,
            # because the model did not pay to generate it.
            "text": PREFILL + tokenizer.decode(generated, skip_special_tokens=True),
            "out_tokens": int(generated.shape[0]),
            "ms": elapsed_ms,
        })
        if (index + 1) % 20 == 0:
            print(f"  {index + 1}/{len(rows)}", flush=True)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(json.dumps(r) for r in written) + "\n", encoding="utf-8")
    print(f"wrote {len(written)} predictions to {out_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
