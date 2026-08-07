"""Evaluate a project's model against held-out eval.jsonl."""

from __future__ import annotations

import json
import os
import re
import urllib.error
import urllib.request

from model_ops.pipeline.jsonl_utils import load_jsonl
from model_ops.pipeline.project_loader import get_project_dir, load_project
from model_ops.registry_hook import register_model_entry


DEFAULT_OLLAMA_BASE = "http://127.0.0.1:11434"


def ollama_api_base() -> str:
    """`OLLAMA_API_BASE`, else Ollama's own default.

    `llm_proxy.core.provider_config` resolved this with dotenv lookup and a
    startup-discovery fallback; the worker is a subprocess of a server that has
    already done that resolution and passes the answer down in the environment,
    so an env read is the whole of it here.
    """
    return (os.environ.get("OLLAMA_API_BASE") or "").strip() or DEFAULT_OLLAMA_BASE


def ollama_chat_url() -> str:
    base = ollama_api_base().rstrip("/")
    return f"{base}/api/chat"


def query_ollama(model: str, user_content: str, timeout: int = 120) -> str | None:
    payload = json.dumps({
        "model": model,
        "stream": False,
        "messages": [{"role": "user", "content": user_content}],
    }).encode()
    req = urllib.request.Request(
        ollama_chat_url(),
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.loads(resp.read().decode())
            return data.get("message", {}).get("content", "")
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
        return None


def score_response(_user_content: str, expected: str, actual: str | None) -> dict:
    if actual is None:
        return {"pass": False, "reason": "no_response", "score": 0.0}

    word_count = len(actual.split())
    too_long = word_count > 100
    too_short = word_count < 5

    actual_lower = actual.lower()
    expected_tokens = set(re.findall(r"[A-Za-z']{4,}", expected.lower()))
    overlap = sum(1 for t in expected_tokens if t in actual_lower)
    keyword_score = min(1.0, overlap / max(1, len(expected_tokens) * 0.3))

    length_score = 0.0 if too_long or too_short else 1.0
    total = keyword_score * 0.7 + length_score * 0.3

    passed = total >= 0.4 and not too_long
    return {
        "pass": passed,
        "score": round(total, 3),
        "word_count": word_count,
        "keyword_score": round(keyword_score, 3),
        "reason": "too_long" if too_long else ("too_short" if too_short else "ok"),
    }


def eval_offline(examples: list[dict]) -> dict:
    results = []
    for ex in examples:
        user = ex["messages"][0]["content"]
        expected = ex["messages"][1]["content"]
        try:
            json.loads(user)
            valid_json = True
        except json.JSONDecodeError:
            valid_json = False
        word_ok = 5 <= len(expected.split()) <= 100
        results.append({
            "pass": valid_json and word_ok,
            "score": 1.0 if (valid_json and word_ok) else 0.0,
            "valid_json": valid_json,
        })
    passed = sum(1 for r in results if r["pass"])
    return {
        "mode": "offline",
        "total": len(results),
        "passed": passed,
        "pass_rate": round(passed / max(1, len(results)), 3),
        "results": results,
    }


def eval_online(examples: list[dict], model: str) -> dict:
    results = []
    for ex in examples:
        user = ex["messages"][0]["content"]
        expected = ex["messages"][1]["content"]
        actual = query_ollama(model, user)
        r = score_response(user, expected, actual)
        r["expected"] = expected[:80]
        r["actual"] = (actual or "")[:120]
        results.append(r)

    passed = sum(1 for r in results if r["pass"])
    avg_score = sum(r["score"] for r in results) / max(1, len(results))
    return {
        "mode": "online",
        "model": model,
        "total": len(results),
        "passed": passed,
        "pass_rate": round(passed / max(1, len(results)), 3),
        "avg_score": round(avg_score, 3),
        "results": results,
    }


def run_eval(project: str, offline: bool = False, model: str | None = None) -> dict:
    project_dir = get_project_dir(project)
    manifest = load_project(project)
    eval_path = project_dir / "datasets" / "eval.jsonl"
    if not eval_path.exists():
        raise FileNotFoundError(f"Missing {eval_path}. Run build_dataset first.")

    examples = load_jsonl(eval_path)
    if offline:
        report = eval_offline(examples)
    else:
        model = model or manifest.get("ollama_tag", project)
        report = eval_online(examples, model)

    report_path = project_dir / "datasets" / "eval_report.json"
    report_path.write_text(json.dumps(report, indent=2), encoding="utf-8")

    if not offline and report.get("avg_score") is not None:
        register_model_entry(
            {
                "ollama_tag": model or manifest.get("ollama_tag", project),
                "project": project,
                "version": manifest.get("version", "v1"),
                "eval_score": report.get("avg_score", report.get("pass_rate")),
            },
            set_active=True,
        )

    return report
