//! The per-project file sandbox — `app/workspace_routes.py` +
//! `app/workspace_service.py`, seven of its eight routes, on both prefixes
//! (`/projects/{id}/workspace/*` and `/projects/{id}/files/*`, the same handlers
//! mounted twice).
//!
//! **All eight routes are here now**, including `POST /upload` and `GET /file`
//! on a `.pdf` — the two that were handed to Python for PyMuPDF. Extraction
//! moved to [`crate::documents`], on a different library, and that module
//! documents exactly how the derived markdown changed as a result.
//!
//! The traversal guard below is the one `todos agent/step` was told not to
//! write twice; it is still the only copy.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{RawQuery, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::wire::{check_len, lax_int, parse_body, required_str};
use crate::{env_opt, AppState};

pub fn routes() -> Router<Arc<AppState>> {
    // The canonical prefix and the legacy alias, which `workspace_routes.py`
    // serves with handlers that call each other. Same handlers here.
    let mut router = Router::new();
    for base in ["/api/v1/projects/{project_id}/workspace", "/api/v1/projects/{project_id}/files"] {
        router = router
            .route(&format!("{base}/info"), get(workspace_info))
            .route(&format!("{base}/ensure-process"), post(ensure_process))
            .route(&format!("{base}/list"), get(workspace_list))
            .route(
                &format!("{base}/file"),
                get(read_file).put(write_file).delete(delete_file),
            )
            .route(&format!("{base}/mkdir"), post(make_dir))
            .route(&format!("{base}/upload"), post(upload_file));
    }
    router
}

// ---------------------------------------------------------------------------
// The sandbox
// ---------------------------------------------------------------------------

/// `WorkspaceError`, which the routes render as `"{code}: {message}"` under
/// their own status.
#[derive(Debug)]
pub(crate) struct WorkspaceError {
    code: &'static str,
    message: String,
    status: StatusCode,
}

impl WorkspaceError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, status: u16) -> Self {
        Self {
            code,
            message: message.into(),
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
        }
    }

    pub(crate) fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, 400)
    }

    /// `getattr(e, "code", str(e))` — what `merge_workspace_documents` puts in
    /// a per-document `error` entry instead of the whole sentence.
    pub(crate) fn code(&self) -> &'static str {
        self.code
    }
}

impl From<WorkspaceError> for ApiError {
    fn from(e: WorkspaceError) -> Self {
        ApiError::new(e.status, format!("{}: {}", e.code, e.message))
    }
}

pub(crate) type WsResult<T> = Result<T, WorkspaceError>;

/// `str(OSError)` — `[WinError 145] The directory is not empty: '<path>'`, or
/// `[Errno 39] Directory not empty: '<path>'` off Windows.
///
/// These reach the client verbatim: `delete_path` and friends put `str(e)` in
/// the message. Rust's own rendering (`… (os error 145)`, no filename) is a
/// different sentence for the same failure, so it is rebuilt here.
fn os_error_text(e: &std::io::Error, path: &Path) -> String {
    let code = e.raw_os_error().unwrap_or_default();
    let raw = e.to_string();
    // Rust appends " (os error N)" and keeps the platform's trailing period;
    // Python's `strerror` has neither.
    let text = raw.split(" (os error ").next().unwrap_or(&raw).trim_end_matches('.');
    let label = if cfg!(windows) { "WinError" } else { "Errno" };
    // The filename goes through `repr()` there, so a Windows path arrives with
    // its backslashes doubled.
    let quoted = path.display().to_string().replace('\\', "\\\\");
    format!("[{label} {code}] {text}: '{quoted}'")
}

const MAX_PATH_SEGMENTS: usize = 32;

pub(crate) fn max_file_bytes() -> u64 {
    env_opt("AGENT_PLATFORM_WORKSPACE_MAX_FILE_BYTES")
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(8 * 1024 * 1024)
}

/// `AGENT_PLATFORM_WORKSPACE_ROOT`, else `<db dir>/workspaces` — with `data` as
/// the directory when the DB path has none.
pub(crate) fn workspace_root() -> PathBuf {
    let root = match env_opt("AGENT_PLATFORM_WORKSPACE_ROOT") {
        Some(explicit) => expanduser(&explicit),
        None => {
            let db = env_opt("AGENT_PLATFORM_DB_PATH")
                .unwrap_or_else(|| "data/agent_platform.db".to_string());
            let parent = Path::new(db.trim()).parent().map(Path::to_path_buf).unwrap_or_default();
            let parent = if parent.as_os_str().is_empty() || parent == Path::new(".") {
                PathBuf::from("data")
            } else {
                parent
            };
            parent.join("workspaces")
        }
    };
    let _ = std::fs::create_dir_all(&root);
    resolve_lexical(&root)
}

fn expanduser(raw: &str) -> PathBuf {
    let raw = raw.trim();
    match raw.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            match env_opt("USERPROFILE").or_else(|| env_opt("HOME")) {
                Some(home) => PathBuf::from(home).join(rest.trim_start_matches(['/', '\\'])),
                None => PathBuf::from(raw),
            }
        }
        _ => PathBuf::from(raw),
    }
}

/// `Path.resolve()` without touching the filesystem.
///
/// **Not `canonicalize`**: on Windows that returns a `\\?\`-prefixed path, and
/// this value is a *response body field* (`absolute_path`) that a user pastes
/// into Explorer. Every relative segment here has already been through
/// [`normalize_relative_path`], which rejects `..`, so a lexical resolve and a
/// real one agree — short of a symlink inside the sandbox, which nothing here
/// creates.
fn resolve_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn project_sandbox_dir(project_id: i64) -> WsResult<PathBuf> {
    if project_id < 1 {
        return Err(WorkspaceError::bad("invalid_project", "project_id must be positive"));
    }
    Ok(workspace_root().join(format!("project-{project_id}")))
}

pub(crate) fn ensure_project_dir(project_id: i64) -> WsResult<PathBuf> {
    let dir = project_sandbox_dir(project_id)?;
    let _ = std::fs::create_dir_all(&dir);
    Ok(resolve_lexical(&dir))
}

/// `normalize_relative_path`: `/`-separated, no `..`, no absolute segments.
/// Empty means the sandbox root.
pub(crate) fn normalize_relative_path(rel: &str) -> WsResult<String> {
    let s = rel.replace('\\', "/");
    let s = s.trim();
    if s.is_empty() || s == "." {
        return Ok(String::new());
    }
    let mut parts: Vec<&str> = Vec::new();
    for segment in s.trim_matches('/').split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(WorkspaceError::bad("invalid_path", "Path must not contain '..'"));
        }
        if segment.chars().count() > 255 {
            return Err(WorkspaceError::bad("invalid_path", "Path segment too long"));
        }
        parts.push(segment);
    }
    if parts.len() > MAX_PATH_SEGMENTS {
        return Err(WorkspaceError::bad(
            "invalid_path",
            format!("Path exceeds {MAX_PATH_SEGMENTS} segments"),
        ));
    }
    Ok(parts.join("/"))
}

/// `_resolve_under_project_for_write`: the target, which need not exist.
pub(crate) fn resolve_for_write(project_id: i64, rel: &str) -> WsResult<PathBuf> {
    let base = ensure_project_dir(project_id)?;
    let normalized = normalize_relative_path(rel)?;
    if normalized.is_empty() {
        return Err(WorkspaceError::bad("invalid_path", "File path must not be empty"));
    }
    let resolved = resolve_lexical(&base.join(normalized.replace('/', std::path::MAIN_SEPARATOR_STR)));
    if !resolved.starts_with(&base) {
        return Err(WorkspaceError::bad("invalid_path", "Path escapes sandbox"));
    }
    Ok(resolved)
}

/// The same, allowing the sandbox root itself.
fn resolve_under(project_id: i64, rel: &str) -> WsResult<PathBuf> {
    let base = ensure_project_dir(project_id)?;
    let normalized = normalize_relative_path(rel)?;
    let target = if normalized.is_empty() {
        base.clone()
    } else {
        base.join(normalized.replace('/', std::path::MAIN_SEPARATOR_STR))
    };
    let resolved = resolve_lexical(&target);
    if resolved != base && !resolved.starts_with(&base) {
        return Err(WorkspaceError::bad("invalid_path", "Path escapes sandbox"));
    }
    Ok(resolved)
}

fn list_dir(project_id: i64, rel: &str) -> WsResult<Vec<Value>> {
    let normalized = normalize_relative_path(rel)?;
    let resolved = resolve_under(project_id, rel)?;
    if !resolved.is_dir() {
        return Err(WorkspaceError::bad("not_a_directory", "Path is not a directory"));
    }
    let entries = std::fs::read_dir(&resolved)
        .map_err(|e| WorkspaceError::new("io_error", os_error_text(&e, &resolved), 500))?;

    // `sorted(key=lambda p: (not p.is_dir(), p.name.lower()))` — directories
    // first, then case-insensitive by name.
    let mut rows: Vec<(bool, String, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_dir() && !kind.is_file() {
            continue;
        }
        rows.push((kind.is_dir(), name.to_lowercase(), name));
    }
    rows.sort_by(|a, b| (!a.0, &a.1).cmp(&(!b.0, &b.1)));

    Ok(rows
        .into_iter()
        .map(|(is_dir, _, name)| {
            let path = if normalized.is_empty() {
                name.clone()
            } else {
                format!("{normalized}/{name}")
            };
            json!({ "name": name, "path": path, "type": if is_dir { "dir" } else { "file" } })
        })
        .collect())
}

pub(crate) fn read_text_file(project_id: i64, rel: &str) -> WsResult<String> {
    let path = resolve_for_write(project_id, rel)?;
    if path.is_dir() {
        return Err(WorkspaceError::bad("is_directory", "Path is a directory"));
    }
    if !path.is_file() {
        return Err(WorkspaceError::new("not_found", "File not found", 404));
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let max = max_file_bytes();
    if size > max {
        return Err(WorkspaceError::new("file_too_large", format!("File exceeds {max} bytes"), 413));
    }
    let raw = std::fs::read(&path)
        .map_err(|e| WorkspaceError::new("io_error", os_error_text(&e, &path), 500))?;
    String::from_utf8(raw)
        .map_err(|_| WorkspaceError::new("not_utf8", "File is not valid UTF-8 text", 415))
}

pub(crate) fn write_text_file(project_id: i64, rel: &str, content: &str) -> WsResult<()> {
    let path = resolve_for_write(project_id, rel)?;
    if path.is_dir() {
        return Err(WorkspaceError::bad("is_directory", "Path is a directory"));
    }
    let max = max_file_bytes();
    if content.len() as u64 > max {
        return Err(WorkspaceError::new(
            "file_too_large",
            format!("Content exceeds {max} bytes"),
            413,
        ));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| WorkspaceError::new("io_error", os_error_text(&e, &path), 500))
}

fn delete_path(project_id: i64, rel: &str) -> WsResult<()> {
    let normalized = normalize_relative_path(rel)?;
    if normalized.is_empty() {
        return Err(WorkspaceError::bad("invalid_path", "Cannot delete sandbox root"));
    }
    let path = resolve_under(project_id, rel)?;
    let base = ensure_project_dir(project_id)?;
    if path == base {
        return Err(WorkspaceError::bad("invalid_path", "Cannot delete sandbox root"));
    }
    if !path.exists() {
        return Err(WorkspaceError::new("not_found", "Path not found", 404));
    }
    if path.is_dir() {
        // `Path.rmdir()` — empty directories only.
        std::fs::remove_dir(&path)
            .map_err(|e| WorkspaceError::bad("directory_not_empty", os_error_text(&e, &path)))
    } else {
        std::fs::remove_file(&path)
            .map_err(|e| WorkspaceError::new("io_error", os_error_text(&e, &path), 500))
    }
}

fn make_directory(project_id: i64, rel: &str) -> WsResult<()> {
    let normalized = normalize_relative_path(rel)?;
    if normalized.is_empty() {
        return Err(WorkspaceError::bad("invalid_path", "Directory path must not be empty"));
    }
    let path = resolve_for_write(project_id, &normalized)?;
    std::fs::create_dir_all(&path)
        .map_err(|e| WorkspaceError::new("io_error", os_error_text(&e, &path), 500))
}

fn ensure_dir_path(project_id: i64, rel: &str) -> WsResult<PathBuf> {
    let normalized = normalize_relative_path(rel)?;
    if normalized.is_empty() {
        return ensure_project_dir(project_id);
    }
    make_directory(project_id, &normalized)?;
    let base = ensure_project_dir(project_id)?;
    let target =
        resolve_lexical(&base.join(normalized.replace('/', std::path::MAIN_SEPARATOR_STR)));
    if !target.starts_with(&base) {
        return Err(WorkspaceError::bad("invalid_path", "Path escapes sandbox"));
    }
    Ok(target)
}

fn process_workspace_rel(process_id: i64) -> WsResult<String> {
    if process_id < 1 {
        return Err(WorkspaceError::bad("invalid_process", "process_id must be positive"));
    }
    Ok(format!("processes/{process_id}"))
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// `_require_project`: the row first, **then** the token's access — so a project
/// that does not exist says so, and one in another workspace says "Not found".
async fn require_project(
    state: &AppState,
    principal: &Principal,
    project_id: i64,
) -> Result<(), ApiError> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM project WHERE id = ?")
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await?;
    if exists.is_none() {
        return Err(ApiError::not_found("Project not found"));
    }
    crate::projects::assert_access(state, principal, project_id).await
}

async fn require_process_for_project(
    state: &AppState,
    project_id: i64,
    process_id: i64,
) -> Result<(), ApiError> {
    let owner: Option<Option<i64>> =
        sqlx::query_scalar("SELECT project_id FROM process WHERE id = ?")
            .bind(process_id)
            .fetch_optional(&state.pool)
            .await?;
    match owner {
        None => Err(ApiError::not_found("Process not found")),
        Some(project) if project == Some(project_id) => Ok(()),
        Some(_) => Err(ApiError::not_found("Process does not belong to this project")),
    }
}

/// A `str` query parameter with a default, the way FastAPI reads one.
fn string_query(query: Option<&str>, name: &str) -> Option<String> {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn require_query(query: Option<&str>, name: &'static str) -> Result<String, ApiError> {
    string_query(query, name).ok_or_else(|| {
        ApiError::validation(vec![json!({
            "type": "missing", "loc": ["query", name], "msg": "Field required",
        })])
    })
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

async fn workspace_info(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    require_project(&state, &principal, project_id).await?;
    let path = string_query(query.as_deref(), "path").unwrap_or_default();

    let normalized = normalize_relative_path(&path)?;
    // A path *inside* a process directory has to name a process of this
    // project — checked before the directory is created, not after.
    if let Some(rest) = normalized.strip_prefix("processes/") {
        let segment = rest.split('/').next().unwrap_or_default();
        if !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()) {
            let process_id = segment.parse::<i64>().unwrap_or_default();
            require_process_for_project(&state, project_id, process_id).await?;
        }
    }

    let absolute = ensure_dir_path(project_id, &path)?;
    Ok(Json(json!({
        "absolute_path": absolute.to_string_lossy(),
        "relative_prefix": normalized,
    }))
    .into_response())
}

async fn ensure_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    require_project(&state, &principal, project_id).await?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let process_id = match body.get("process_id") {
        None => {
            errors.push(ApiError::field_error("process_id", "missing", "Field required"));
            0
        }
        Some(_) => {
            let parsed = lax_int(&mut errors, &body, "process_id").unwrap_or_default();
            if errors.is_empty() && parsed < 1 {
                errors.push(ApiError::field_error(
                    "process_id",
                    "greater_than_equal",
                    "Input should be greater than or equal to 1",
                ));
            }
            parsed
        }
    };
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    require_process_for_project(&state, project_id, process_id).await?;
    let rel = process_workspace_rel(process_id)?;
    make_directory(project_id, &rel)?;
    let base = ensure_project_dir(project_id)?;
    let absolute = resolve_lexical(&base.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)));

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "ok": true,
            "absolute_path": absolute.to_string_lossy(),
            "relative_prefix": rel,
        })),
    )
        .into_response())
}

async fn workspace_list(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    require_project(&state, &principal, project_id).await?;
    let path = string_query(query.as_deref(), "path").unwrap_or_default();
    let entries = list_dir(project_id, &path)?;
    Ok(Json(json!({ "entries": entries })).into_response())
}

/// `GET /file` — `read_workspace_file_for_llm`, PDFs included.
///
/// A `.pdf` resolves through its derived markdown, extracting on the spot if
/// that file is not there yet, which is why this route can *write*. That was
/// the reason it stayed with Python: the miss path is an ingest.
async fn read_file(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    request: Request,
) -> Result<Response, ApiError> {
    let query = request.uri().query().map(str::to_string);
    let path = require_query(query.as_deref(), "path")?;
    require_project(&state, &principal, project_id).await?;
    Ok(Json(crate::documents::read_for_llm(project_id, &path)?).into_response())
}

/// `POST /upload` — multipart, one `file` part, `dest` from the query string
/// (default `documents`).
async fn upload_file(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    RawQuery(query): RawQuery,
    mut multipart: axum::extract::Multipart,
) -> Result<Response, ApiError> {
    require_project(&state, &principal, project_id).await?;
    let dest = string_query(query.as_deref(), "dest").unwrap_or_else(|| "documents".to_string());

    // `file: UploadFile = File(...)` — the part is required and named `file`;
    // anything else in the body is ignored, as FastAPI ignores it.
    let mut found: Option<(String, Vec<u8>)> = None;
    loop {
        let field = multipart
            .next_field()
            .await
            .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("read_failed: {e}")))?;
        let Some(field) = field else { break };
        if field.name() != Some("file") {
            continue;
        }
        // `(file.filename or "upload").strip()`.
        let name = field.file_name().unwrap_or("upload").trim().to_string();
        let name = if name.is_empty() { "upload".to_string() } else { name };
        let data = field
            .bytes()
            .await
            .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("read_failed: {e}")))?;
        found = Some((name, data.to_vec()));
        break;
    }

    let Some((filename, data)) = found else {
        return Err(ApiError::validation(vec![ApiError::field_error(
            "file",
            "missing",
            "Field required",
        )]));
    };

    let result = crate::documents::ingest_upload(project_id, &filename, &data, &dest)?;
    Ok(Json(result.to_json()).into_response())
}

async fn write_file(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    require_project(&state, &principal, project_id).await?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let path = required_str(&mut errors, &body, "path");
    if body.get("path").is_some_and(Value::is_string) {
        check_len(&mut errors, &["path"], Some(path.as_str()), 1, 8192);
    }
    let content = crate::wire::defaulted_str(&mut errors, &body, "content", "");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    write_text_file(project_id, &path, &content)?;
    // The path echoed back is the caller's, not the normalised one.
    Ok(Json(json!({ "ok": true, "path": path })).into_response())
}

async fn delete_file(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    require_project(&state, &principal, project_id).await?;
    let path = require_query(query.as_deref(), "path")?;
    delete_path(project_id, &path)?;
    Ok(Json(json!({ "ok": true })).into_response())
}

async fn make_dir(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    require_project(&state, &principal, project_id).await?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let path = required_str(&mut errors, &body, "path");
    if body.get("path").is_some_and(Value::is_string) {
        check_len(&mut errors, &["path"], Some(path.as_str()), 1, 8192);
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    make_directory(project_id, &path)?;
    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "path": path }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_is_rejected_before_anything_touches_the_disk() {
        assert!(normalize_relative_path("../etc/passwd").is_err());
        assert!(normalize_relative_path("a/../../b").is_err());
        // Backslashes are separators, not literal characters.
        assert_eq!(normalize_relative_path("a\\b\\c").unwrap(), "a/b/c");
        assert_eq!(normalize_relative_path("/leading/slash/").unwrap(), "leading/slash");
        assert_eq!(normalize_relative_path("  ").unwrap(), "");
        assert_eq!(normalize_relative_path(".").unwrap(), "");
        assert_eq!(normalize_relative_path("a//b/./c").unwrap(), "a/b/c");
    }

    #[test]
    fn path_limits_match_the_service() {
        let deep = (0..33).map(|i| i.to_string()).collect::<Vec<_>>().join("/");
        let err = normalize_relative_path(&deep).unwrap_err();
        assert_eq!(err.code, "invalid_path");
        assert!(err.message.contains("32 segments"));

        let long = "x".repeat(256);
        assert_eq!(normalize_relative_path(&long).unwrap_err().message, "Path segment too long");
        // 255 is the boundary and is allowed.
        assert!(normalize_relative_path(&"x".repeat(255)).is_ok());
    }

    #[test]
    fn a_lexical_resolve_keeps_the_windows_path_a_user_can_paste() {
        // `canonicalize` would return a `\\?\`-prefixed path here; this is a
        // response body field, so it must not.
        let resolved = resolve_lexical(Path::new("/base/./a/b"));
        assert!(!resolved.to_string_lossy().contains("?"));
        assert!(resolved.ends_with("a/b") || resolved.ends_with("a\\b"));
    }

    #[test]
    fn the_error_shape_is_code_colon_message() {
        let api: ApiError = WorkspaceError::new("not_found", "File not found", 404).into();
        assert_eq!(api.status, StatusCode::NOT_FOUND);
        assert_eq!(api.message, "not_found: File not found");
    }
}
