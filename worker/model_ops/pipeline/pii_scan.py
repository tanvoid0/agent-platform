"""Refuse to build a dataset that still carries personal data.

ADR 0015 §2.4, layer L4. This is the platform's own check and it deliberately
does **not** trust the client that uploaded the knowledge files. A client that
masks correctly still has to pass it; a client that forgot, or a new client
nobody has reviewed, is stopped here rather than at the weights.

It works on shapes, never on literals, and that is a design constraint rather
than a shortcut: an exact check would mean the platform holding the very values
it is protecting, which is one more copy of them in one more place. Shapes cost
some false positives and catch data the platform was never told about.

**What is not scanned for: money.** A job advert says "£55,000 - £65,000" and
that is the training signal, not a leak. Salary is checked upstream, in the
exporter that does hold the record and can compare literals (L3). A pattern
here would fail every honest dataset in this domain, and a gate that always
fires gets switched off.

The findings never quote what they matched. A scanner that prints the email
address it found has written it into the job log, which is the file this exists
to keep clean.
"""

from __future__ import annotations

import re
from typing import Iterable, NamedTuple

# Each pattern is anchored on structure that free prose does not produce by
# accident. Ordered most to least specific so the first hit on a line is the
# most informative one.
PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("email", re.compile(r"\b[\w.+-]+@[\w-]+\.[\w.-]{2,}\b")),
    # UK National Insurance: two letters, six digits, a final letter A-D.
    ("national_insurance", re.compile(r"\b[A-CEGHJ-PR-TW-Z]{2}\s?\d{2}\s?\d{2}\s?\d{2}\s?[A-D]\b")),
    # UK postcode, full form only. The outward code alone ("SW1") is a place
    # name in an advert; the full unit identifies a household.
    ("postcode", re.compile(r"\b[A-Z]{1,2}\d[A-Z\d]?\s?\d[A-Z]{2}\b", re.IGNORECASE)),
    # +44…, 07…, and the grouped forms, 10 digits or more once punctuation is
    # discounted. Deliberately not matching 4-6 digit extensions.
    ("phone", re.compile(r"(?<![\d.])(?:\+\d{1,3}[\s-]?)?(?:\(?\d{2,5}\)?[\s-]?){2,4}\d{3,4}(?![\d.])")),
    # Card and account numbers: an unbroken run long enough that nothing in
    # prose reaches it. Version strings and years do not.
    ("long_number", re.compile(r"(?<![\d.])\d{12,}(?![\d.])")),
)

# A phone pattern that loose matches things that are not phone numbers. These
# are the shapes seen in this corpus that would otherwise fire on every file.
_PHONE_FALSE_POSITIVES = re.compile(
    r"""
    ^(?:                       # the whole match, not a fragment of it
        \d{4}\s*[-–]\s*\d{4}   # a date range, "2019 - 2023"
      | \d{1,2}[/.]\d{1,2}[/.]\d{2,4}   # a date
    )$
    """,
    re.VERBOSE,
)


class Finding(NamedTuple):
    kind: str
    line: int
    length: int

    def describe(self) -> str:
        # No excerpt, by design — see the module docstring.
        return f"line {self.line}: {self.kind} ({self.length} chars)"


def _is_false_positive(kind: str, matched: str) -> bool:
    if kind == "phone":
        stripped = matched.strip()
        if _PHONE_FALSE_POSITIVES.match(stripped):
            return True
        # Fewer than 10 digits is an extension, a reference number, or a range.
        return sum(char.isdigit() for char in stripped) < 10
    return False


def scan_text(text: str, line_offset: int = 0) -> list[Finding]:
    findings: list[Finding] = []
    for index, line in enumerate(text.splitlines(), start=1 + line_offset):
        for kind, pattern in PATTERNS:
            for match in pattern.finditer(line):
                if _is_false_positive(kind, match.group(0)):
                    continue
                findings.append(Finding(kind, index, len(match.group(0))))
    return findings


def scan_rows(rows: Iterable[dict]) -> list[Finding]:
    """Scan chat rows. The row index is the reported line number."""
    findings: list[Finding] = []
    for index, row in enumerate(rows, start=1):
        for message in row.get("messages", []) or []:
            content = message.get("content")
            if isinstance(content, str):
                findings.extend(
                    Finding(f.kind, index, f.length) for f in scan_text(content)
                )
    return findings


def require_clean(rows: Iterable[dict], where: str, limit: int = 20) -> None:
    """Raise unless the rows are clean. The build stops here.

    Hard failure rather than dropping the offending rows: a silent drop leaves
    a dataset that is smaller than the operator thinks and a masker bug nobody
    finds out about. The point of the gate is that somebody looks.
    """
    findings = scan_rows(rows)
    if not findings:
        return

    kinds = sorted({f.kind for f in findings})
    shown = "\n  ".join(f.describe() for f in findings[:limit])
    more = f"\n  … and {len(findings) - limit} more" if len(findings) > limit else ""
    raise ValueError(
        f"Personal data found in {where}: {len(findings)} match(es) of {', '.join(kinds)}.\n"
        f"  {shown}{more}\n"
        "Mask it at the source that produced these rows, not here — a dataset is\n"
        "copied into weights that cannot be edited afterwards. Set `pii_scan: false`\n"
        "in project.yaml only if this corpus is public data with no subject in it."
    )
