"""
Extract structured text from PDF bytes for LLM / workspace tools.

Requires ``pymupdf`` (``fitz``). When missing, callers should surface a clear error.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class PdfExtractResult:
    markdown: str
    page_count: int
    title: str | None
    extractor: str


def pymupdf_available() -> bool:
    try:
        import fitz  # noqa: F401

        return True
    except ImportError:
        return False


def extract_pdf_structured_markdown(data: bytes, *, max_pages: int | None = None) -> PdfExtractResult:
    """
    Turn PDF bytes into page-marked markdown suitable for planner / chat context.
    """
    try:
        import fitz
    except ImportError as e:
        raise RuntimeError(
            "PDF extraction requires pymupdf (pip install pymupdf). "
            "Upload saved; re-read after installing the dependency."
        ) from e

    doc = fitz.open(stream=data, filetype="pdf")
    try:
        page_count = doc.page_count
        limit = page_count if max_pages is None else min(page_count, max(0, max_pages))
        title = (doc.metadata or {}).get("title") or None
        if title is not None:
            title = str(title).strip() or None

        parts: list[str] = []
        if title:
            parts.append(f"# {title}\n")
        parts.append(f"_PDF document ({page_count} page{'s' if page_count != 1 else ''})_\n")

        for i in range(limit):
            page = doc.load_page(i)
            text = page.get_text("text") or ""
            text = text.strip()
            parts.append(f"\n## Page {i + 1}\n\n")
            if text:
                parts.append(text)
                parts.append("\n")
            else:
                parts.append("_(no extractable text on this page — may be scanned/image-only)_\n")

            blocks = page.get_text("blocks")
            if blocks:
                table_lines = _blocks_to_table_hints(blocks)
                if table_lines:
                    parts.append("\n### Layout notes\n\n")
                    parts.extend(table_lines)
                    parts.append("\n")

        if limit < page_count:
            parts.append(
                f"\n---\n_(Truncated to first {limit} of {page_count} pages in this extract.)_\n"
            )

        markdown = "".join(parts).strip() or "_(empty PDF)_"
        return PdfExtractResult(
            markdown=markdown,
            page_count=page_count,
            title=title,
            extractor="pymupdf",
        )
    finally:
        doc.close()


def _blocks_to_table_hints(blocks: list) -> list[str]:
    """Lightweight hints from text blocks (positions) without full table OCR."""
    lines: list[str] = []
    seen = 0
    for b in blocks[:40]:
        if len(b) < 5:
            continue
        x0, y0, x1, y1, text, *_ = b
        t = (text or "").strip().replace("\n", " ")
        if len(t) < 4:
            continue
        if len(t) > 200:
            t = t[:200] + "…"
        lines.append(f"- block @ ({int(x0)},{int(y0)}): {t}")
        seen += 1
        if seen >= 12:
            break
    return lines
