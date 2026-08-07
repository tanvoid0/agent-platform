"""Export LoRA adapter to GGUF and create Ollama model."""

from __future__ import annotations

import os
import shutil
import subprocess
from datetime import datetime, timezone
from pathlib import Path


from model_ops.pipeline.project_loader import (
    get_ollama_base_model,
    get_project_dir,
    load_project,
    require_base_model,
)
from model_ops.registry_hook import register_model_entry


def run(cmd: list[str], cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)


def find_llama_convert() -> Path | None:
    env_path = os.environ.get("LLAMA_CPP_CONVERT", "").strip()
    if env_path and Path(env_path).exists():
        return Path(env_path)
    for candidate in (
        shutil.which("convert_hf_to_gguf.py"),
        Path(r"C:\tools\llama.cpp\convert_hf_to_gguf.py"),
    ):
        if candidate and Path(candidate).exists():
            return Path(candidate)
    return None


def find_llama_quantize() -> Path | None:
    env_bin = os.environ.get("LLAMA_CPP_BIN", "").strip()
    if env_bin:
        q = Path(env_bin) / "llama-quantize.exe"
        if q.exists():
            return q
        q2 = Path(env_bin) / "llama-quantize"
        if q2.exists():
            return q2
    for candidate in (
        shutil.which("llama-quantize"),
        Path(r"C:\tools\llama.cpp\build\bin\Release\llama-quantize.exe"),
        Path(r"C:\tools\llama.cpp\build\bin\llama-quantize.exe"),
    ):
        if candidate and Path(candidate).exists():
            return Path(candidate)
    return None


def quant_type_for_llama_quantize(quant: str) -> str:
    return quant.upper().replace("-", "_")


def convert_merged_to_gguf(merged_dir: Path, gguf_path: Path, quant: str) -> tuple[bool, str]:
    llama_convert = find_llama_convert()
    if not llama_convert or not merged_dir.exists():
        return False, "llama.cpp convert_hf_to_gguf.py not found or merged dir missing"

    direct_types = {"f32", "f16", "bf16", "q8_0", "tq1_0", "tq2_0", "auto"}
    if quant in direct_types:
        result = run(
            ["python", str(llama_convert), str(merged_dir), "--outfile", str(gguf_path), "--outtype", quant]
        )
        if result.returncode == 0:
            return True, f"GGUF written to {gguf_path}"
        return False, f"GGUF conversion failed: {result.stderr or result.stdout}"

    f16_path = gguf_path.with_name(gguf_path.stem + "-f16.gguf")
    result = run(
        ["python", str(llama_convert), str(merged_dir), "--outfile", str(f16_path), "--outtype", "f16"]
    )
    if result.returncode != 0:
        return False, f"F16 GGUF conversion failed: {result.stderr or result.stdout}"

    llama_quantize = find_llama_quantize()
    if not llama_quantize:
        return False, f"F16 GGUF at {f16_path}, but llama-quantize not found."

    qtype = quant_type_for_llama_quantize(quant)
    result = run([str(llama_quantize), str(f16_path), str(gguf_path), qtype])
    if result.returncode == 0:
        return True, f"GGUF written to {gguf_path} (quantized {qtype})"
    return False, f"Quantization failed: {result.stderr or result.stdout}"


def merge_and_export_gguf(
    project: str,
    adapter_version: str = "v1",
    quant: str = "q4_k_m",
    skip_merge: bool = False,
    ollama_create_fn=None,
) -> Path:
    manifest = load_project(project)
    project_dir = get_project_dir(project)
    adapter_dir = project_dir / "adapters" / adapter_version
    export_dir = project_dir / "export"
    export_dir.mkdir(parents=True, exist_ok=True)

    base_model = require_base_model(manifest, project)
    ollama_base = get_ollama_base_model()
    ollama_tag = manifest.get("ollama_tag", project)
    gguf_name = f"{ollama_tag}-{quant}.gguf"
    gguf_path = export_dir / gguf_name
    merged_dir = export_dir / "merged"

    if not adapter_dir.exists():
        raise FileNotFoundError(f"Adapter not found: {adapter_dir}. Run train first.")

    steps: list[str] = []

    if not skip_merge:
        try:
            import torch
            from peft import PeftModel
            from transformers import AutoModelForCausalLM, AutoTokenizer

            tokenizer = AutoTokenizer.from_pretrained(base_model, trust_remote_code=True)
            base = AutoModelForCausalLM.from_pretrained(
                base_model,
                torch_dtype=torch.bfloat16,
                device_map="cpu",
                trust_remote_code=True,
            )
            model = PeftModel.from_pretrained(base, str(adapter_dir))
            model = model.merge_and_unload()
            merged_dir.mkdir(parents=True, exist_ok=True)
            model.save_pretrained(str(merged_dir))
            tokenizer.save_pretrained(str(merged_dir))
            steps.append(f"Merged weights saved to {merged_dir}")
        except ImportError:
            steps.append("Install torch/peft/transformers to auto-merge.")

    if merged_dir.exists():
        ok, msg = convert_merged_to_gguf(merged_dir, gguf_path, quant)
        steps.append(msg)
    else:
        gguf_path = export_dir / f"{ollama_tag}-q4.gguf"
        steps.append("GGUF conversion not run (merged weights missing).")

    modelfile_src = export_dir / "Modelfile"
    system_path = project_dir / manifest.get("system_prompt", "export/system.txt")
    system_text = system_path.read_text(encoding="utf-8").strip() if system_path.exists() else ""

    if gguf_path.exists():
        from_line = f"FROM ./{gguf_path.name}"
    else:
        from_line = f"FROM {ollama_base}"
        steps.append(f"Using base {ollama_base} in Modelfile until custom GGUF is available.")

    modelfile_content = f"""{from_line}

PARAMETER temperature 0.4
PARAMETER top_p 0.9
PARAMETER num_ctx 4096

SYSTEM \"\"\"{system_text}\"\"\"
"""
    modelfile_src.write_text(modelfile_content, encoding="utf-8")

    if ollama_create_fn is not None:
        ok, msg = ollama_create_fn(ollama_tag, modelfile_src.read_text(encoding="utf-8"))
        steps.append(msg if msg else ("Ollama model created." if ok else "Ollama create failed."))
    elif shutil.which("ollama"):
        result = run(["ollama", "create", ollama_tag, "-f", str(modelfile_src)])
        if result.returncode == 0:
            steps.append(f"Ollama model '{ollama_tag}' created.")
        else:
            steps.append(f"ollama create failed: {result.stderr or result.stdout}")
    else:
        steps.append("Ollama not in PATH.")

    instructions_path = export_dir / "EXPORT_INSTRUCTIONS.md"
    instructions_path.write_text(
        f"# Export instructions for {project}\n\nGenerated: {datetime.now(timezone.utc).isoformat()}\n\n"
        + "\n".join(f"- {s}" for s in steps),
        encoding="utf-8",
    )

    rel_gguf = str(gguf_path.relative_to(project_dir)) if gguf_path.exists() else None
    register_model_entry(
        {
            "ollama_tag": ollama_tag,
            "project": project,
            "version": adapter_version,
            "base_model": base_model,
            "adapter": f"projects/{project}/adapters/{adapter_version}",
            "gguf": rel_gguf,
            "created_at": datetime.now(timezone.utc).strftime("%Y-%m-%d"),
            "eval_score": None,
        },
        set_active=True,
    )
    return modelfile_src
