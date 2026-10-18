#!/usr/bin/env python3
"""Fail when a request struct and its `openapi.json` schema disagree on fields.

`tests/openapi_drift.rs` checks one direction: every documented operation reaches
a handler. It says nothing about *shapes*, so a new field on a request struct — or
a property the document invents — goes unnoticed. `tools` was added to
`SendRequest` and nothing would have complained had the spec never learned about
it. With two clients (the iced app and `portal_desktop`) the spec is the contract,
so the shapes need a check too.

This is deliberately not a Rust test. Enumerating a struct's serde fields at
runtime needs either reflection or `utoipa` annotations on 141 paths, and
`lib.rs::openapi` already records that the hand-maintained document won that
argument. Reading the field names out of the source is the cheap approximation,
and being a lint rather than a test is what keeps its brittleness out of the
compile.

Scope is the request bodies the two clients actually post. Widen PAIRS when a
third client appears rather than trying to cover every schema — an unchecked
schema is a known gap, an unmaintainable check is a worse one.

Run: python scripts/check_openapi_request_drift.py
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "desktop" / "crates" / "server" / "src"

# (rust file, rust struct, openapi component schema, deliberately-undocumented).
#
# The fourth column is for a field the wire accepts but the route ignores. Those
# are real — `RetryRequest` and `ApprovalRequest` flatten a whole `SendRequest`,
# so they accept every field it has and honour a subset. The spec documents what
# is honoured, which is the contract worth publishing; see
# docs/coder-delegation-protocol.md. Listing them here is what stops that
# intent reading as drift, and what makes adding to the list a visible decision.
PAIRS = [
    ("coder.rs", "SendRequest", "CoderChatSendRequest", set()),
    ("coder.rs", "RetryRequest", "CoderRetryRequest", {"message"}),
    (
        "coder.rs",
        "ApprovalRequest",
        "CoderApprovalRequest",
        {"message", "workspace_root", "allow_commands", "plan"},
    ),
    ("coder.rs", "ToolResultRequest", "CoderToolResultRequest", set()),
    ("coder.rs", "ThreadCreateRequest", "CoderThreadCreateRequest", set()),
    ("media.rs", "GenerateRequest", "MediaGenerateRequest", set()),
    ("ads.rs", "CampaignCreate", "AdCampaignCreate", set()),
    # `Brand` is both the request body and the response of PUT /brand — the
    # bare object either way, so one schema covers both directions.
    ("ads.rs", "Brand", "AdBrand", set()),
]

_SOURCES: dict[str, str] = {}


def source(name: str) -> str:
    if name not in _SOURCES:
        _SOURCES[name] = (SRC / name).read_text(encoding="utf-8")
    return _SOURCES[name]


def serde_fields(file: str, struct: str, seen: set[str] | None = None) -> list[str] | None:
    """Field names as serde sees them, following `#[serde(flatten)]` inward.

    Returns None when the struct is not found at all, which is a failure worth
    reporting rather than an empty diff that silently passes.
    """
    seen = seen or set()
    if struct in seen:
        return []
    seen.add(struct)

    body = re.search(r"struct " + re.escape(struct) + r"\s*\{(.*?)\n\}", source(file), re.S)
    if not body:
        return None

    out: list[str] = []
    flatten_next = False
    for raw in body.group(1).split("\n"):
        line = raw.strip()
        if line.startswith("#["):
            flatten_next = "flatten" in line
            continue
        if not line or line.startswith("//"):
            continue
        match = re.match(r"(?:pub(?:\(crate\))?\s+)?([a-z_][a-z0-9_]*)\s*:\s*(.+?),?$", line)
        if not match:
            continue
        if flatten_next:
            inner = match.group(2).strip().rstrip(",")
            inner = re.sub(r"^Option<(.*)>$", r"\1", inner).strip()
            nested = serde_fields(file, inner, seen)
            if nested is None:
                out.append(f"<unresolved flatten: {inner}>")
            else:
                out.extend(nested)
        else:
            out.append(match.group(1))
        flatten_next = False
    return out


def main() -> int:
    doc = json.loads((SRC / "openapi.json").read_text(encoding="utf-8"))
    schemas = doc["components"]["schemas"]

    problems: list[str] = []
    for file, struct, schema, ignored in PAIRS:
        rust = serde_fields(file, struct)
        if rust is None:
            problems.append(f"{struct}: no such struct in {file} (renamed? then update PAIRS)")
            continue
        if schema not in schemas:
            problems.append(f"{schema}: not in openapi.json components.schemas")
            continue

        documented = set(schemas[schema].get("properties", {}).keys())
        actual = set(rust)

        undocumented = sorted(actual - documented - ignored)
        phantom = sorted(documented - actual)
        # An entry that stopped being ignored — the field went away, so the
        # allowlist is now lying about why it is there.
        stale = sorted(ignored - actual)

        for field in undocumented:
            problems.append(
                f"{struct}.{field} is accepted by the server but absent from {schema}. "
                f"Document it, or add it to this script's ignore set with a reason."
            )
        for field in phantom:
            problems.append(f"{schema}.{field} is documented but {struct} has no such field.")
        for field in stale:
            problems.append(
                f"{struct} no longer has `{field}`, but this script still lists it as "
                f"deliberately ignored. Drop it from PAIRS."
            )

    if problems:
        print("openapi.json and the request structs disagree:\n", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    print(f"openapi request schemas match their structs ({len(PAIRS)} checked)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
