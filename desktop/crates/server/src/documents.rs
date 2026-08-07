//! Workspace document ingest — `app/document_service.py` and the
//! `app/pdf_extraction.py` under it.
//!
//! Uploads land in the project sandbox as-is; a PDF additionally gets a derived
//! markdown rendering at `<path>.derived/structured.md` and a manifest beside
//! it, so an agent can `workspace_read` a PDF and get text back.
//!
//! # The extractor is not PyMuPDF any more
//!
//! Python used `fitz`, and this is the one place in the whole migration where
//! no Rust equivalent exists — ADR 0007 listed PDF extraction under "staying
//! Python permanently" for exactly that reason. Retiring the interpreter
//! overrides that, so the extractor is now the `pdf-extract` crate and **the
//! markdown a given PDF produces is not byte-identical to what it produced
//! before.** The document *shape* is preserved (title heading, page count line,
//! `## Page N` sections, the scanned-page notice, the truncation footer),
//! because that shape is what the excerpt and the chat context are built from.
//! What differs:
//!
//! - **Word and line breaks inside a page.** Two text extractors disagree about
//!   where a space goes; nothing downstream parses this, it is read by a model.
//! - **`### Layout notes` are gone.** They came from PyMuPDF's per-block
//!   bounding boxes (`page.get_text("blocks")`), which `pdf-extract` does not
//!   expose. They were "lightweight hints without full table OCR" — a dozen
//!   `- block @ (x,y): …` lines meant to help a model guess at table structure.
//!   ponytail: re-derivable from `lopdf` (already in the tree, `pdf-extract`
//!   re-exports it) by walking the content stream for text-positioning
//!   operators; worth doing only if table-heavy PDFs measurably degrade.
//! - **The `extractor` field reads `pdf-extract`**, not `pymupdf`, and the
//!   manifest's `pymupdf_available` flag is now `extractor_available` — a
//!   compiled-in crate is always available, so it is a constant `true` kept for
//!   the manifest's shape rather than a real probe.
//!
//! Re-ingesting an existing document overwrites its derived markdown, so a
//! workspace carrying PyMuPDF-era output converges on the new extractor the
//! next time anything re-reads it.

use std::path::Path;

use serde_json::{json, Value};

use crate::workspace_files::{
    ensure_project_dir, max_file_bytes, normalize_relative_path, read_text_file, resolve_for_write,
    write_text_file, WorkspaceError, WsResult,
};

const PDF_SUFFIX: &str = ".pdf";
const DERIVED_DIR_NAME: &str = ".derived";
const STRUCTURED_MD: &str = "structured.md";
const MANIFEST_JSON: &str = "manifest.json";
const MAX_CHAT_EXCERPT_CHARS: usize = 24_000;
/// `ALLOWED_UPLOAD_SUFFIXES`, in the sorted order the 415 message renders them.
const ALLOWED_UPLOAD_SUFFIXES: [&str; 4] = [".markdown", ".md", ".pdf", ".txt"];

/// `DocumentIngestResult`.
pub(crate) struct Ingested {
    pub path: String,
    pub mime_type: &'static str,
    pub bytes_written: usize,
    pub derived_path: Option<String>,
    pub manifest_path: Option<String>,
    pub page_count: Option<usize>,
    pub excerpt: String,
    pub extraction: &'static str,
}

impl Ingested {
    /// The upload route's body.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "mime_type": self.mime_type,
            "bytes": self.bytes_written,
            "derived_path": self.derived_path,
            "manifest_path": self.manifest_path,
            "page_count": self.page_count,
            "excerpt": self.excerpt,
            "extraction": self.extraction,
        })
    }
}

fn derived_prefix_for_file(rel: &str) -> WsResult<String> {
    Ok(format!("{}{DERIVED_DIR_NAME}", normalize_relative_path(rel)?))
}

fn structured_derived_path(rel: &str) -> WsResult<String> {
    Ok(format!("{}/{STRUCTURED_MD}", derived_prefix_for_file(rel)?))
}

fn manifest_derived_path(rel: &str) -> WsResult<String> {
    Ok(format!("{}/{MANIFEST_JSON}", derived_prefix_for_file(rel)?))
}

/// `_safe_upload_basename`: the leaf name, with anything outside
/// `[\w.\- ]` collapsed to `_`.
///
/// `\w` in Python's `re` is Unicode-aware by default, so an accented filename
/// keeps its letters rather than being punched full of underscores — hence the
/// `is_alphanumeric` here rather than an ASCII test.
fn safe_upload_basename(name: &str) -> WsResult<String> {
    let base = Path::new(name).file_name().map(|s| s.to_string_lossy().into_owned());
    let base = base.unwrap_or_default();
    let base = base.trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err(WorkspaceError::bad("invalid_filename", "Filename is required"));
    }

    // `re.sub(r"[^\w.\- ]+", "_", base)` — a *run* of disallowed characters
    // collapses to a single underscore, not one each.
    let mut out = String::with_capacity(base.len());
    let mut in_run = false;
    for c in base.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || c == ' ' {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        return Err(WorkspaceError::bad("invalid_filename", "Filename is invalid"));
    }
    Ok(out)
}

fn suffix_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

fn mime_for_suffix(suffix: &str) -> &'static str {
    match suffix {
        PDF_SUFFIX => "application/pdf",
        ".md" | ".markdown" => "text/markdown",
        _ => "text/plain",
    }
}

/// `ingest_workspace_upload`.
pub(crate) fn ingest_upload(
    project_id: i64,
    filename: &str,
    data: &[u8],
    dest_dir: &str,
) -> WsResult<Ingested> {
    let max = max_file_bytes();
    if data.len() as u64 > max {
        return Err(WorkspaceError::new(
            "file_too_large",
            format!("File exceeds {max} bytes"),
            413,
        ));
    }
    if data.is_empty() {
        return Err(WorkspaceError::bad("empty_file", "File is empty"));
    }

    let safe_name = safe_upload_basename(filename)?;
    let suffix = suffix_of(&safe_name);
    if !ALLOWED_UPLOAD_SUFFIXES.contains(&suffix.as_str()) {
        return Err(WorkspaceError::new(
            "unsupported_type",
            format!("Supported uploads: {}", ALLOWED_UPLOAD_SUFFIXES.join(", ")),
            415,
        ));
    }

    let dir_rel = normalize_relative_path(dest_dir)?;
    let dir_rel = if dir_rel.is_empty() { "documents".to_string() } else { dir_rel };
    let rel = normalize_relative_path(&format!("{dir_rel}/{safe_name}"))?;

    let path = resolve_for_write(project_id, &rel)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, data)
        .map_err(|e| WorkspaceError::new("io_error", e.to_string(), 500))?;

    let mime = mime_for_suffix(&suffix);
    if suffix == PDF_SUFFIX {
        return ingest_pdf(project_id, &rel, data, mime);
    }

    // `data.decode("utf-8")` — a non-UTF-8 upload raises, after the bytes are
    // already on disk. Same order here: the file is saved, then the excerpt
    // fails, which is what makes "upload saved, re-read after fixing" true.
    let text = String::from_utf8(data.to_vec())
        .map_err(|_| WorkspaceError::new("not_utf8", "File is not valid UTF-8 text", 415))?;
    Ok(Ingested {
        path: rel,
        mime_type: mime,
        bytes_written: data.len(),
        derived_path: None,
        manifest_path: None,
        page_count: None,
        excerpt: clip_excerpt(&text),
        extraction: "utf8",
    })
}

/// `_ingest_pdf`: extract, write the derived markdown and manifest, return the
/// excerpt. An extraction failure is **not** an error — it writes a markdown
/// stub explaining itself, because the PDF is already saved and the caller's
/// upload succeeded.
fn ingest_pdf(project_id: i64, rel: &str, data: &[u8], mime: &'static str) -> WsResult<Ingested> {
    let structured_rel = structured_derived_path(rel)?;
    let manifest_rel = manifest_derived_path(rel)?;

    let (markdown, page_count, extraction, error) = match extract_pdf_markdown(data) {
        Ok(extracted) => (extracted.markdown, Some(extracted.page_count), EXTRACTOR, None),
        Err(e) => {
            let name = Path::new(rel).file_name().map(|s| s.to_string_lossy().into_owned());
            let name = name.unwrap_or_else(|| rel.to_string());
            let stub = format!(
                "# PDF: {name}\n\n_Extraction failed: {e}_\n\n\
                 The original PDF was saved in the workspace. \
                 Paste the text into chat if you need it read."
            );
            (stub, None, "failed", Some(e))
        }
    };

    write_text_file(project_id, &structured_rel, &markdown)?;
    let manifest = json!({
        "source_path": rel,
        "derived_structured": structured_rel,
        "mime_type": mime,
        "page_count": page_count,
        "extractor": extraction,
        "extractor_available": true,
        "error": error,
        "updated_at": crate::request_id::iso_now(),
    });
    write_text_file(
        project_id,
        &manifest_rel,
        &serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".into()),
    )?;

    Ok(Ingested {
        path: rel.to_string(),
        mime_type: mime,
        bytes_written: data.len(),
        derived_path: Some(structured_rel),
        manifest_path: Some(manifest_rel),
        page_count,
        excerpt: clip_excerpt(&markdown),
        extraction,
    })
}

/// `read_workspace_file_for_llm`: UTF-8 text, or a PDF through its derived
/// markdown — extracting on the spot when the derived file is not there yet.
pub(crate) fn read_for_llm(project_id: i64, rel: &str) -> WsResult<Value> {
    let normalized = normalize_relative_path(rel)?;
    if normalized.is_empty() {
        return Err(WorkspaceError::bad("invalid_path", "path is required"));
    }

    if !normalized.to_lowercase().ends_with(PDF_SUFFIX) {
        return Ok(json!({
            "path": normalized,
            "content": read_text_file(project_id, &normalized)?,
            "content_kind": "text",
            "derived_path": Value::Null,
        }));
    }

    let structured = structured_derived_path(&normalized)?;
    let base = ensure_project_dir(project_id)?;
    let derived_file = base.join(structured.replace('/', std::path::MAIN_SEPARATOR_STR));
    if derived_file.is_file() {
        return Ok(json!({
            "path": normalized,
            "content": read_text_file(project_id, &structured)?,
            "content_kind": "pdf_derived_markdown",
            "derived_path": structured,
        }));
    }

    let raw_path = resolve_for_write(project_id, &normalized)?;
    if !raw_path.is_file() {
        return Err(WorkspaceError::new("not_found", "File not found", 404));
    }
    let data = std::fs::read(&raw_path)
        .map_err(|e| WorkspaceError::new("io_error", e.to_string(), 500))?;
    let max = max_file_bytes();
    if data.len() as u64 > max {
        return Err(WorkspaceError::new(
            "file_too_large",
            format!("File exceeds {max} bytes"),
            413,
        ));
    }
    let result = ingest_pdf(project_id, &normalized, &data, "application/pdf")?;
    let derived = result.derived_path.clone().unwrap_or(structured);
    Ok(json!({
        "path": normalized,
        "content": read_text_file(project_id, &derived)?,
        "content_kind": "pdf_derived_markdown",
        "derived_path": result.derived_path,
    }))
}

/// `_clip_excerpt`. Counted in **characters**, as Python's `len(str)` is.
fn clip_excerpt(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_CHAT_EXCERPT_CHARS {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX_CHAT_EXCERPT_CHARS - 80).collect();
    format!(
        "{}\n\n---\n_(Excerpt truncated for chat; use workspace_read on the derived path for \
         full text.)_",
        head.trim_end()
    )
}

// ---------------------------------------------------------------------------
// The extractor — `pdf_extraction.py`
// ---------------------------------------------------------------------------

const EXTRACTOR: &str = "pdf-extract";

/// `PdfExtractResult`, minus the `title` field nothing outside this module read.
#[derive(Debug)]
struct PdfExtracted {
    markdown: String,
    page_count: usize,
}

/// `extract_pdf_structured_markdown`. `Err` carries the sentence that ends up
/// in the manifest's `error` and in the stub markdown.
fn extract_pdf_markdown(data: &[u8]) -> Result<PdfExtracted, String> {
    let pages = pdf_extract::extract_text_from_mem_by_pages(data)
        .map_err(|e| format!("could not read the PDF: {e}"))?;
    let page_count = pages.len();
    let title = pdf_title(data);

    let mut out = String::new();
    if let Some(title) = &title {
        out.push_str(&format!("# {title}\n\n"));
    }
    out.push_str(&format!(
        "_PDF document ({page_count} page{})_\n",
        if page_count == 1 { "" } else { "s" }
    ));

    for (index, text) in pages.iter().enumerate() {
        out.push_str(&format!("\n## Page {}\n\n", index + 1));
        let text = text.trim();
        if text.is_empty() {
            out.push_str("_(no extractable text on this page — may be scanned/image-only)_\n");
        } else {
            out.push_str(text);
            out.push('\n');
        }
    }

    // Python capped at `max_pages` and appended a truncation footer; every
    // caller passed `None`, so the cap never fired and the footer never
    // rendered. Not carried over — an unreachable branch is not a behaviour.

    let markdown = out.trim().to_string();
    let markdown = if markdown.is_empty() { "_(empty PDF)_".to_string() } else { markdown };
    Ok(PdfExtracted { markdown, page_count })
}

/// `doc.metadata["title"]`, empty-string-as-absent like Python's
/// `str(title).strip() or None`.
///
/// A document that fails to open here is not an error: the page text already
/// extracted, and a missing heading is not worth failing an ingest over.
fn pdf_title(data: &[u8]) -> Option<String> {
    let doc = pdf_extract::Document::load_mem(data).ok()?;
    let info = doc.trailer.get(b"Info").ok()?;
    let info = info.as_reference().ok().and_then(|id| doc.get_object(id).ok()).or(Some(info))?;
    let title = info.as_dict().ok()?.get(b"Title").ok()?;
    let title = title.as_str().ok()?;
    // PDF text strings are either PDFDocEncoding or UTF-16BE with a BOM.
    let decoded = if title.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = title[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).ok()?
    } else {
        String::from_utf8_lossy(title).into_owned()
    };
    let decoded = decoded.trim();
    (!decoded.is_empty()).then(|| decoded.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `re.sub` semantics that are easy to get wrong: a *run* of bad
    /// characters is one underscore, and the result is trimmed afterwards.
    #[test]
    fn upload_names_are_sanitized_like_the_python_regex() {
        assert_eq!(safe_upload_basename("report.pdf").unwrap(), "report.pdf");
        assert_eq!(safe_upload_basename("/etc/passwd").unwrap(), "passwd");
        assert_eq!(safe_upload_basename("a/b/c/notes.md").unwrap(), "notes.md");
        assert_eq!(safe_upload_basename("bad***name.txt").unwrap(), "bad_name.txt");
        assert_eq!(safe_upload_basename("  spaced out.txt  ").unwrap(), "spaced out.txt");
        // `\w` is Unicode-aware, so accented letters survive.
        assert_eq!(safe_upload_basename("résumé.pdf").unwrap(), "résumé.pdf");
        // A name that is nothing but disallowed characters collapses to `_`,
        // which is not empty — so it is accepted, and then rejected for its
        // (absent) suffix rather than as an invalid filename.
        assert_eq!(safe_upload_basename("***").unwrap(), "_");

        for bad in ["", "   ", ".", ".."] {
            assert!(safe_upload_basename(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn derived_paths_sit_beside_the_source() {
        assert_eq!(
            structured_derived_path("documents/report.pdf").unwrap(),
            "documents/report.pdf.derived/structured.md"
        );
        assert_eq!(
            manifest_derived_path("documents/report.pdf").unwrap(),
            "documents/report.pdf.derived/manifest.json"
        );
    }

    #[test]
    fn suffix_and_mime_agree_with_the_allowlist() {
        assert_eq!(suffix_of("Report.PDF"), ".pdf");
        assert_eq!(suffix_of("no-extension"), "");
        assert_eq!(mime_for_suffix(".pdf"), "application/pdf");
        assert_eq!(mime_for_suffix(".markdown"), "text/markdown");
        assert_eq!(mime_for_suffix(".txt"), "text/plain");
    }

    /// The excerpt clip is measured in characters, so a multi-byte document
    /// must not be cut by byte offset — that would panic on a char boundary.
    #[test]
    fn excerpts_clip_by_character() {
        let short = "  hello  ";
        assert_eq!(clip_excerpt(short), "hello");

        let long = "é".repeat(MAX_CHAT_EXCERPT_CHARS + 500);
        let clipped = clip_excerpt(&long);
        assert!(clipped.ends_with("full text.)_"), "footer missing");
        let head_len = clipped.chars().take_while(|c| *c == 'é').count();
        assert_eq!(head_len, MAX_CHAT_EXCERPT_CHARS - 80);

        // Exactly at the limit is not truncated.
        let exact = "x".repeat(MAX_CHAT_EXCERPT_CHARS);
        assert_eq!(clip_excerpt(&exact), exact);
    }

    /// A PDF that cannot be parsed must fail as a *message*, not a panic — the
    /// ingest path turns that message into the stub markdown.
    #[test]
    fn garbage_bytes_fail_with_a_sentence() {
        let err = extract_pdf_markdown(b"not a pdf at all").unwrap_err();
        assert!(err.starts_with("could not read the PDF: "), "{err}");
        assert!(pdf_title(b"not a pdf at all").is_none());
    }

    /// End to end on a real (minimal, hand-built) PDF: one page, known text.
    /// Without this the extractor is only tested on the failure path.
    #[test]
    fn a_real_pdf_renders_the_document_shape() {
        let pdf = minimal_pdf("Hello from a test PDF");
        let extracted = extract_pdf_markdown(&pdf).expect("minimal PDF should parse");
        assert_eq!(extracted.page_count, 1);
        assert!(extracted.markdown.contains("_PDF document (1 page)_"), "{}", extracted.markdown);
        assert!(extracted.markdown.contains("## Page 1"), "{}", extracted.markdown);
        assert!(
            extracted.markdown.contains("Hello from a test PDF"),
            "text missing: {}",
            extracted.markdown
        );
    }

    /// A single-page PDF drawing `text` in Helvetica. Written by hand rather
    /// than checked in as a fixture so the offsets in the xref table stay
    /// correct if this is ever edited.
    fn minimal_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];

        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
        }

        let xref_at = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}
