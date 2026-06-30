"""Build interactive planning forms from clarifying-question actions."""

from __future__ import annotations

import re
from typing import Any

_MAX_FIELDS = 12
_MAX_OPTION_LEN = 120
_VALID_KINDS = frozenset(
    {"boolean", "single_select", "multi_select", "text", "textarea"}
)


def _slug_id(index: int, label: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "_", label.lower()).strip("_")[:40]
    return slug or f"q{index}"


def _parse_paren_options(question: str) -> list[str] | None:
    m = re.search(r"\(([^)]+)\)\??\s*$", question.strip())
    if not m:
        return None
    inner = m.group(1).strip()
    inner = re.sub(r"^e\.g\.?,?\s*", "", inner, flags=re.I).strip()
    if not inner or len(inner) > 200:
        return None
    parts = re.split(r",|/|\s+or\s+", inner, flags=re.I)
    opts = [p.strip().strip(".") for p in parts if p.strip()]
    opts = [o[:_MAX_OPTION_LEN] for o in opts if o]
    if 2 <= len(opts) <= 8:
        return opts
    return None


def _prefer_or_options(question: str) -> list[str] | None:
    lower = question.lower()
    if "prefer" not in lower or " or " not in lower:
        return None
    parts = re.split(r"\s+or\s+", question, maxsplit=1, flags=re.I)
    if len(parts) != 2:
        return None
    left = re.sub(r"^.*\bprefer\b", "", parts[0], flags=re.I).strip(" ?:.,")
    right = parts[1].strip(" ?:.,")
    opts = [o for o in (left, right) if o and len(o) < 120]
    return opts if len(opts) == 2 else None


def _infer_field_kind(question: str, options: list[str] | None) -> str:
    if options:
        lower = question.lower()
        if any(
            x in lower
            for x in ("any of", "which of", "select all", "avoid", "restrictions", "allerg")
        ):
            return "multi_select"
        return "single_select"
    lower = question.lower().strip()
    if re.match(r"^are there\b", lower) or re.match(r"^what\b", lower):
        return "textarea"
    if re.match(
        r"^(do|does|did|is|can|will|should|would|have you|has)\b",
        lower,
    ) and " or " not in lower and "how many" not in lower:
        return "boolean"
    if "how many" in lower or re.search(r"\b\d+\s*(days?|meals?|times?)\b", lower):
        return "text"
    if len(question) > 100:
        return "textarea"
    return "text"


def _coerce_llm_field(raw: dict[str, Any], index: int) -> dict[str, Any] | None:
    label = raw.get("label") or raw.get("question")
    if not isinstance(label, str) or not label.strip():
        return None
    label = label.strip()[:200]
    fid = raw.get("id")
    if not isinstance(fid, str) or not fid.strip():
        fid = _slug_id(index, label)
    else:
        fid = re.sub(r"[^a-zA-Z0-9_]", "_", fid.strip())[:40]

    kind = raw.get("kind")
    if not isinstance(kind, str) or kind not in _VALID_KINDS:
        kind = _infer_field_kind(label, None)

    field: dict[str, Any] = {
        "id": fid,
        "label": label,
        "kind": kind,
        "required": raw.get("required") is True,
    }
    if isinstance(raw.get("helpText"), str) and raw["helpText"].strip():
        field["helpText"] = raw["helpText"].strip()[:400]

    options = raw.get("options")
    if kind in ("single_select", "multi_select"):
        if isinstance(options, list) and len(options) >= 2:
            field["options"] = [
                str(o).strip()[:_MAX_OPTION_LEN]
                for o in options
                if o is not None and str(o).strip()
            ][:8]
        else:
            parsed = _parse_paren_options(label)
            if parsed:
                field["options"] = parsed
            else:
                field["kind"] = "text"
    return field


def _field_from_question(
    question: str,
    index: int,
    *,
    profile: dict[str, Any] | None = None,
) -> dict[str, Any]:
    q = question.strip()
    label = re.sub(r"\s*\([^)]+\)\s*$", "", q).strip() or q
    options = _parse_paren_options(q) or _prefer_or_options(q)
    kind = _infer_field_kind(q, options)
    fid = _slug_id(index, label)

    field: dict[str, Any] = {
        "id": fid,
        "label": label[:200],
        "kind": kind,
        "required": False,
    }
    if options and kind in ("single_select", "multi_select"):
        field["options"] = options

    if profile:
        val = profile.get(fid)
        if val is None:
            for key, pv in profile.items():
                if key.lower() in label.lower() or label.lower() in key.replace("_", " "):
                    val = pv
                    break
        if val is not None and val != "" and val != []:
            field["default"] = val

    return field


def build_clarifying_form(
    questions: list[str],
    *,
    title: str | None = None,
    llm_fields: list[dict[str, Any]] | None = None,
    profile: dict[str, Any] | None = None,
) -> dict[str, Any] | None:
    """Turn clarifying question strings into a PlanningFormSpec-compatible dict."""
    if llm_fields:
        fields: list[dict[str, Any]] = []
        seen: set[str] = set()
        for i, raw in enumerate(llm_fields[:_MAX_FIELDS]):
            if not isinstance(raw, dict):
                continue
            f = _coerce_llm_field(raw, i)
            if not f or f["id"] in seen:
                continue
            seen.add(f["id"])
            if profile and "default" not in f:
                val = profile.get(f["id"])
                if val is not None and val != "" and val != []:
                    f["default"] = val
            fields.append(f)
        if fields:
            return {
                "purpose": "clarifying",
                "title": (title or "A few quick questions").strip()[:200],
                "description": "Pick the options that fit you, or type where needed.",
                "fields": fields,
            }

    qs = [q.strip() for q in questions if q and str(q).strip()]
    if not qs:
        return None
    fields = [
        _field_from_question(q, i, profile=profile)
        for i, q in enumerate(qs[:_MAX_FIELDS])
    ]
    return {
        "purpose": "clarifying",
        "title": (title or "A few quick questions").strip()[:200],
        "description": "Pick the options that fit you, or type where needed.",
        "fields": fields,
    }


def is_clarifying_form(form: dict[str, Any] | None) -> bool:
    return isinstance(form, dict) and form.get("purpose") == "clarifying"


def format_clarifying_answers_message(
    form: dict[str, Any], answers: dict[str, Any]
) -> str:
    """User message sent after the clarifying form is submitted."""
    fields = form.get("fields") if isinstance(form.get("fields"), list) else []
    id_to_label = {
        f.get("id"): f.get("label", f.get("id"))
        for f in fields
        if isinstance(f, dict) and f.get("id")
    }
    lines = ["My answers to your questions:", ""]
    for key, v in answers.items():
        label = id_to_label.get(key, key.replace("_", " "))
        if isinstance(v, list):
            val = ", ".join(str(x) for x in v) if v else "(none)"
        elif isinstance(v, bool):
            val = "Yes" if v else "No"
        else:
            val = str(v).strip() or "(skipped)"
        lines.append(f"- {label}: {val}")
    lines.append("")
    lines.append("Please continue planning using these answers.")
    return "\n".join(lines)
