//! Tool execution for the Coder agent — `app/coder/executor.py` plus
//! `desktop_executor.py`.
//!
//! Two executors, picked per turn exactly as `make_executor` picks them:
//!
//! - [`Executor::Local`] runs the tools on *this* machine, jailed to one
//!   workspace root. It is `LocalExecutor`, string for string: a model reads
//!   `Error: File not found: src/ap.rs` and tries `src/app.rs` next, so every
//!   failure is a tool *result* rather than an error that ends the turn.
//! - [`Executor::Delegated`] is `DesktopDelegatedExecutor`: the turn parks and
//!   the desktop runs the call, posting the answer back to
//!   `POST /coder/chat/tool-result`.
//!
//! **The park is the reason this domain moved in one commit.** Python's park is
//! a module-level `dict[(thread_id, call_id), asyncio.Future]`; here it is
//! [`crate::AppState::coder_pending`], a map of `oneshot::Sender`. Either way it
//! is *process memory*: the unpark has to land in the same process that served
//! `/chat/stream`. Split the two across servers and `/chat/tool-result` 404s
//! while the turn waits out its full 300 seconds and then feeds the model
//! "timed out" — a wrong answer rather than a failure.
//!
//! Rust is strictly better than Python on one detail here and it is
//! unobservable: `/chat/tool-result` is a sync `def` there, so it calls
//! `set_result` on an asyncio future from a threadpool thread without
//! `call_soon_threadsafe` — the loop is never woken and the resume waits for
//! the next poll. `oneshot::Sender::send` wakes the parked task by
//! construction.
//!
//! `coder_tools.rs` in the *app* crate is the other half of the delegated path
//! and stays there: the same six tools with the same wording, run on the
//! desktop. The one deliberate difference is `run_command`'s timeout — 180s
//! there against Python's 60s here, and here is where parity is measured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::AppState;

/// `MAX_READ_BYTES`.
const MAX_READ_BYTES: usize = 256 * 1024;
/// `MAX_DIR_ENTRIES`.
const MAX_DIR_ENTRIES: usize = 500;
/// `LocalExecutor(command_timeout_seconds=60.0)`. The number reaches the model
/// in the timeout message, so it is part of the contract, not a knob.
const COMMAND_TIMEOUT_SECONDS: u64 = 60;
/// `desktop_executor.execute`'s `asyncio.wait_for(..., timeout=300.0)`.
pub(crate) const DELEGATION_TIMEOUT_SECONDS: u64 = 300;

const SEARCH_MAX_FILE_BYTES: u64 = 1_000_000;
const SEARCH_MAX_HITS: usize = 100;
const SEARCH_MAX_HIT_CHARS: usize = 300;
const MAP_MAX_FILES: usize = 400;
/// `SKIP_DIRS`. Dot-*directories* are skipped by rule; dot files are project
/// config someone may well be after.
const SKIP_DIRS: [&str; 7] =
    ["node_modules", "target", "dist", "build", "__pycache__", "venv", "site-packages"];

pub(crate) const PORTAL_DESKTOP_CLIENT_ID: &str = "portal-desktop";

pub(crate) fn is_portal_desktop_client(client_id: Option<&str>) -> bool {
    client_id.unwrap_or_default().trim().eq_ignore_ascii_case(PORTAL_DESKTOP_CLIENT_ID)
}

/// `ToolExecutionError` — a failure the *caller* sees, as opposed to the ones
/// `execute` swallows into a tool result. Only executor construction raises
/// one that escapes, and its route answers 400 with this text.
pub(crate) struct ToolExecutionError(pub String);

pub(crate) enum Executor {
    Local { root: PathBuf, allow_commands: bool },
    /// `_allow_commands` is assigned and never read in Python — the desktop
    /// owns that decision — so it is not carried here either.
    Delegated { thread_id: i64, root: String },
}

/// `make_executor`: the client header or an explicit `delegate_tools` picks the
/// desktop; anything else runs here.
pub(crate) fn make_executor(
    workspace_root: &str,
    thread_id: i64,
    client_id: Option<&str>,
    allow_commands: bool,
    delegate_tools: bool,
) -> Result<Executor, ToolExecutionError> {
    if is_portal_desktop_client(client_id) || delegate_tools {
        let root = workspace_root.trim();
        if root.is_empty() {
            return Err(ToolExecutionError(
                "workspace_root is required for desktop-delegated execution".to_string(),
            ));
        }
        return Ok(Executor::Delegated { thread_id, root: root.to_string() });
    }
    let root = resolve_root(workspace_root).ok_or_else(|| {
        ToolExecutionError(format!("Workspace root is not a directory: {workspace_root}"))
    })?;
    Ok(Executor::Local { root, allow_commands })
}

/// `Path(workspace_root).expanduser().resolve()`, refusing anything that is not
/// a directory.
///
/// The verbatim prefix is stripped because `str(Path.resolve())` on Windows is
/// `D:\work`, not `\\?\D:\work`, and that string is **persisted** on the thread
/// row and returned in every response body.
fn resolve_root(workspace_root: &str) -> Option<PathBuf> {
    let expanded = expanduser(workspace_root.trim());
    let resolved = std::fs::canonicalize(&expanded).ok()?;
    let resolved = strip_verbatim(resolved);
    resolved.is_dir().then_some(resolved)
}

/// `~` and `~/rest` only. Python's `expanduser` also expands `~other-user`,
/// which needs the platform's password database; a path spelled that way falls
/// through unchanged and then fails the is-a-directory check, which is the same
/// 400 a nonexistent root gets.
fn expanduser(raw: &str) -> PathBuf {
    let Some(rest) = raw.strip_prefix('~') else { return PathBuf::from(raw) };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return PathBuf::from(raw);
    }
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return PathBuf::from(raw);
    };
    let trimmed = rest.trim_start_matches(['/', '\\']);
    if trimmed.is_empty() {
        PathBuf::from(home)
    } else {
        PathBuf::from(home).join(trimmed)
    }
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        // A UNC verbatim path (`\\?\UNC\server\share`) has no plain spelling,
        // so it is left alone rather than mangled into `UNC\server\share`.
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

impl Executor {
    /// `str(executor.workspace_root)` — what the turn persists on the thread.
    pub(crate) fn workspace_root_string(&self) -> String {
        match self {
            Executor::Local { root, .. } => root.to_string_lossy().into_owned(),
            Executor::Delegated { root, .. } => root.clone(),
        }
    }

    pub(crate) async fn execute(
        &self,
        state: &Arc<AppState>,
        tool: &str,
        args: &Value,
        call_id: &str,
    ) -> String {
        match self {
            Executor::Local { root, allow_commands } => {
                execute_local(root, tool, args, *allow_commands).await
            }
            Executor::Delegated { thread_id, .. } => {
                execute_delegated(state, *thread_id, call_id).await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Delegation — the parked call
// ---------------------------------------------------------------------------

/// Whoever parked clears the key on every path, including a cancelled turn, so
/// a later call with the same id is never refused by a corpse.
struct ParkGuard {
    state: Arc<AppState>,
    key: (i64, String),
}

impl Drop for ParkGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.state.coder_pending.lock() {
            pending.remove(&self.key);
        }
    }
}

async fn execute_delegated(state: &Arc<AppState>, thread_id: i64, call_id: &str) -> String {
    if call_id.is_empty() {
        return "Error: internal error: missing call_id for desktop tool delegation".to_string();
    }
    let key = (thread_id, call_id.to_string());
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    {
        let Ok(mut pending) = state.coder_pending.lock() else {
            return format!("Error: duplicate tool call id {call_id}");
        };
        if pending.contains_key(&key) {
            // Python returns this *as the tool result* rather than failing the
            // turn, so the model sees it and moves on.
            return format!("Error: duplicate tool call id {call_id}");
        }
        pending.insert(key.clone(), tx);
    }
    let _guard = ParkGuard { state: state.clone(), key };

    let timeout = std::time::Duration::from_secs(DELEGATION_TIMEOUT_SECONDS);
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => result,
        // The sender was dropped without answering — unreachable while the
        // guard above is the only remover, and Python's future would simply
        // never resolve. Treated as the timeout it effectively is.
        Ok(Err(_)) | Err(_) => "Error: timed out waiting for desktop to execute tool".to_string(),
    }
}

/// `resolve_desktop_tool_result`. A missing or already-resolved key is the
/// `KeyError` the route turns into a 404 — the text is the detail.
pub(crate) fn resolve_desktop_tool_result(
    state: &AppState,
    thread_id: i64,
    call_id: &str,
    result: String,
) -> Result<(), String> {
    let absent = || {
        format!(
            "No pending desktop tool call for thread={thread_id} call_id={}",
            crate::todos::py_repr(&Value::String(call_id.to_string()))
        )
    };
    let mut pending = state.coder_pending.lock().map_err(|_| absent())?;
    let tx = pending.remove(&(thread_id, call_id.to_string())).ok_or_else(absent)?;
    // The receiver is gone only if the turn already moved on, which is the
    // "already done" half of Python's check.
    tx.send(result).map_err(|_| absent())
}

pub(crate) type PendingMap = HashMap<(i64, String), tokio::sync::oneshot::Sender<String>>;

// ---------------------------------------------------------------------------
// LocalExecutor
// ---------------------------------------------------------------------------

/// `LocalExecutor.execute`'s dispatch, including its two blanket `except`s:
/// a `ToolExecutionError` and an `OSError` both come back as `Error: {e}`
/// rather than raised, so the model can correct itself.
async fn execute_local(root: &Path, tool: &str, args: &Value, allow_commands: bool) -> String {
    // `str(args.get(k, ""))` — a non-string argument is stringified Python's
    // way, not JSON's, because that is what reaches the filesystem.
    let arg = |k: &str| match args.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => crate::todos::py_repr(other),
        None => String::new(),
    };
    match tool {
        "read_file" => read_file(root, &arg("path")),
        "write_file" => write_file(root, &arg("path"), &arg("content")),
        "list_dir" => {
            let path = match args.get("path") {
                None => ".".to_string(),
                _ => arg("path"),
            };
            list_dir(root, &path)
        }
        "search" => search(root, &arg("query")),
        "repo_map" => repo_map(root),
        "run_command" => run_command(root, &arg("command"), allow_commands).await,
        other => format!("Error: unknown tool '{other}'."),
    }
}

/// `LocalExecutor._resolve`.
///
/// Canonicalizing is the whole check: it resolves `..` and follows symlinks, so
/// a link inside the root pointing at `~/.ssh` is caught by where it *lands*
/// rather than by how it is spelled. A file that does not exist yet cannot be
/// canonicalized, so the walk drops components until something does and
/// re-attaches the rest — by then `..` is already gone.
fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let raw = rel.trim();
    let raw = if raw.is_empty() { "." } else { raw };
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() { candidate.to_path_buf() } else { root.join(candidate) };

    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe = joined;
    let resolved = loop {
        if let Ok(found) = std::fs::canonicalize(&probe) {
            break strip_verbatim(found);
        }
        match (probe.file_name().map(|n| n.to_os_string()), probe.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                probe = parent.to_path_buf();
            }
            _ => {
                return Err(format!(
                    "Error: Path escapes the workspace root and was blocked: {rel}"
                ))
            }
        }
    };
    let target = tail.iter().rev().fold(resolved, |p, name| p.join(name));
    if target.starts_with(root) {
        Ok(target)
    } else {
        Err(format!("Error: Path escapes the workspace root and was blocked: {rel}"))
    }
}

fn read_file(root: &Path, rel: &str) -> String {
    let path = match resolve_in_root(root, rel) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !path.is_file() {
        return format!("Error: File not found: {rel}");
    }
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => return format!("Error: {e}"),
    };
    let total = data.len();
    // `errors="replace"`: a stray byte costs one character, not the whole read.
    let mut text = String::from_utf8_lossy(&data[..total.min(MAX_READ_BYTES)]).into_owned();
    if total > MAX_READ_BYTES {
        text.push_str(&format!("\n...[truncated: file is {total} bytes]"));
    }
    text
}

fn write_file(root: &Path, rel: &str, content: &str) -> String {
    if rel.trim().is_empty() {
        return "Error: write_file requires a non-empty path".to_string();
    }
    let path = match resolve_in_root(root, rel) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if path == root || path.is_dir() {
        return format!("Error: Path is a directory, not a file: {rel}");
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("Error: {e}");
        }
    }
    match std::fs::write(&path, content) {
        // `len(content.encode("utf-8"))` — bytes, which is what `str::len` is.
        Ok(()) => format!("Wrote {} bytes to {rel}", content.len()),
        Err(e) => format!("Error: {e}"),
    }
}

fn list_dir(root: &Path, rel: &str) -> String {
    let path = match resolve_in_root(root, rel) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let read = match std::fs::read_dir(&path) {
        Ok(r) => r,
        Err(_) => return format!("Error: Directory not found: {rel}"),
    };
    let mut entries: Vec<(bool, String)> = read
        .filter_map(|e| e.ok())
        .map(|e| {
            let dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (dir, e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    // `key=lambda c: (not c.is_dir(), c.name.lower())` — directories first,
    // then case-insensitively by name.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));

    if entries.is_empty() {
        return "(empty directory)".to_string();
    }
    let truncated = entries.len() > MAX_DIR_ENTRIES;
    let mut out: Vec<String> = entries
        .into_iter()
        .take(MAX_DIR_ENTRIES)
        .map(|(dir, name)| if dir { name + "/" } else { name })
        .collect();
    if truncated {
        out.push(format!("...[truncated at {MAX_DIR_ENTRIES} entries]"));
    }
    out.join("\n")
}

/// `_walk_files`: every file under the root, in a stable order, skipping build
/// output. Stable is the point — the caps below cut a walk off part-way, so an
/// arbitrary order would return a different hundred hits each call.
fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut children: Vec<std::fs::DirEntry> = read.filter_map(|e| e.ok()).collect();
        children.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            // `file_type` rather than `is_dir`: it does not follow symlinks, so
            // a link pointing back up the tree cannot become an endless descent.
            match child.file_type() {
                Ok(t) if t.is_dir() => {
                    if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                        subdirs.push(child.path());
                    }
                }
                Ok(t) if t.is_file() => out.push(child.path()),
                _ => {}
            }
        }
        // Reversed, because the stack pops from the end.
        stack.extend(subdirs.into_iter().rev());
    }
    out
}

/// `_rel`: relative to the root, forward slashes on every platform — the way
/// the model has to spell it back to `read_file`.
fn rel_posix(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// A failed UTF-8 decode **is** the binary sniffer, exactly as in Python —
/// hence `from_utf8` rather than the lossy read `read_file` uses.
fn read_text(path: &Path, max_bytes: u64) -> Option<String> {
    if path.metadata().ok()?.len() > max_bytes {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

fn search(root: &Path, query: &str) -> String {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return "Error: search requires a non-empty query".to_string();
    }
    let mut hits: Vec<String> = Vec::new();
    let mut truncated = false;
    'files: for path in walk_files(root) {
        if hits.len() >= SEARCH_MAX_HITS {
            truncated = true;
            break;
        }
        let Some(text) = read_text(&path, SEARCH_MAX_FILE_BYTES) else { continue };
        let rel = rel_posix(root, &path);
        for (i, line) in text.lines().enumerate() {
            if hits.len() >= SEARCH_MAX_HITS {
                truncated = true;
                break 'files;
            }
            if line.to_lowercase().contains(&needle) {
                let shown: String = line.trim().chars().take(SEARCH_MAX_HIT_CHARS).collect();
                hits.push(format!("{rel}:{}: {shown}", i + 1));
            }
        }
    }
    if hits.is_empty() {
        // `f"no matches for {query.strip()!r}"` — a repr, so a query with an
        // apostrophe in it comes back double-quoted.
        return format!("no matches for {}", crate::todos::py_repr(&Value::String(query.trim().to_string())));
    }
    if truncated {
        hits.push(format!("...[truncated at {SEARCH_MAX_HITS} matches — narrow the query]"));
    }
    hits.join("\n")
}

/// Extension → (keywords that declare a name, whether `export` is required).
/// `_MAP_LANGUAGES`.
fn map_language(ext: &str) -> Option<(&'static [&'static str], bool)> {
    const PY: &[&str] = &["def", "class"];
    const RS: &[&str] =
        &["fn", "struct", "enum", "trait", "type", "const", "static", "mod", "macro_rules!"];
    const JS: &[&str] = &["function", "class", "const", "let", "var", "interface", "type", "enum"];
    match ext {
        "py" | "pyi" => Some((PY, false)),
        "rs" => Some((RS, false)),
        // JS/TS: only exported names, which are the ones importers use.
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => Some((JS, true)),
        _ => None,
    }
}

/// `_MAP_MODIFIERS`. `pub(crate)` is matched by prefix.
const MAP_MODIFIERS: [&str; 7] =
    ["pub", "async", "unsafe", "extern", "default", "declare", "abstract"];

/// `definition_name`. Only column 0 counts, in every language: a Rust `impl`
/// block's methods and a Python class's methods are detail, and the map answers
/// "where does this name live", not "what is in this file".
fn definition_name(line: &str, keywords: &[&str], require_export: bool) -> Option<String> {
    if line.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let mut tokens = line.split_whitespace().peekable();
    if require_export && tokens.next_if_eq(&"export").is_none() {
        return None;
    }
    while tokens.next_if(|t| MAP_MODIFIERS.contains(t) || t.starts_with("pub(")).is_some() {}
    let keyword = tokens.next()?.trim_end_matches('*'); // `function*`
    if !keywords.contains(&keyword) {
        return None;
    }
    let name: String = tokens
        .next()?
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn repo_map(root: &Path) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut truncated = false;
    for path in walk_files(root) {
        let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
        let Some((keywords, require_export)) = map_language(&ext) else { continue };
        if scanned >= MAP_MAX_FILES {
            truncated = true;
            break;
        }
        scanned += 1;
        // No size cap: Python reads the file outright, and a source file past a
        // megabyte is generated anyway.
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let mut names: Vec<String> = Vec::new();
        for line in text.lines() {
            if let Some(name) = definition_name(line, keywords, require_export) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        // A file with nothing top-level is dropped rather than listed empty:
        // `list_dir` already says what exists, and this answers a different
        // question.
        if !names.is_empty() {
            lines.push(format!("{}: {}", rel_posix(root, &path), names.join(", ")));
        }
    }
    if lines.is_empty() {
        return "no definitions found — this workspace may not be Python, Rust or \
                JavaScript/TypeScript, or its code may be somewhere list_dir has not reached"
            .to_string();
    }
    lines.sort();
    if truncated {
        lines.push(format!("...[truncated at {MAP_MAX_FILES} files — use search instead]"));
    }
    std::iter::once("definitions by file:".to_string()).chain(lines).collect::<Vec<_>>().join("\n")
}

/// `_run_command`. `shell=True` is `cmd /C` on Windows and `/bin/sh -c`
/// everywhere else, which is what `subprocess` picks.
async fn run_command(root: &Path, command: &str, allow_commands: bool) -> String {
    if !allow_commands {
        return "Error: command execution is disabled for this session. \
                Ask the user to enable it (allow_commands) if a command is required."
            .to_string();
    }
    if command.trim().is_empty() {
        return "Error: run_command requires a non-empty command".to_string();
    }

    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash
    cmd.current_dir(root).stdin(std::process::Stdio::null()).kill_on_drop(true);

    let timeout = std::time::Duration::from_secs(COMMAND_TIMEOUT_SECONDS);
    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => return format!("Error: command timed out after {COMMAND_TIMEOUT_SECONDS}s"),
        // `subprocess.run` raises OSError when the shell cannot start, which
        // `execute`'s `except OSError` turns into this.
        Ok(Err(e)) => return format!("Error: {e}"),
        Ok(Ok(out)) => out,
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    format!("[exit code {}]\n{text}", out.status.code().unwrap_or(-1)).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("server-coder-tools-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        strip_verbatim(std::fs::canonicalize(&dir).unwrap())
    }

    #[test]
    fn a_path_climbing_out_of_the_root_is_refused() {
        let root = scratch("escape");
        std::fs::create_dir_all(root.join("src")).unwrap();
        assert!(resolve_in_root(&root, "src/app.rs").is_ok());
        // The file need not exist yet — write_file's whole job is creating one.
        assert!(resolve_in_root(&root, "src/new/deep/file.rs").is_ok());

        for escape in ["../secrets.env", "src/../../secrets.env", "src/../.."] {
            let err = resolve_in_root(&root, escape).unwrap_err();
            assert!(err.contains("escapes the workspace root"), "{escape} was allowed: {err}");
        }
        assert!(resolve_in_root(&root, "/etc/passwd").is_err());
        assert!(resolve_in_root(&root, root.join("src").to_str().unwrap()).is_ok());
    }

    #[test]
    fn a_sibling_root_sharing_a_prefix_is_not_inside() {
        let root = scratch("prefix");
        let sibling = scratch("prefix-evil");
        std::fs::write(sibling.join("x"), "x").unwrap();
        assert!(
            resolve_in_root(&root, sibling.join("x").to_str().unwrap()).is_err(),
            "component-wise containment, not string prefix"
        );
    }

    /// Every string here was read off `coder/executor.py`, not reasoned about:
    /// the model sees these verbatim and recovers from them.
    #[test]
    fn results_read_the_way_pythons_local_executor_words_them() {
        let root = scratch("rw");
        assert_eq!(write_file(&root, "src/a.txt", "hello"), "Wrote 5 bytes to src/a.txt");
        assert_eq!(read_file(&root, "src/a.txt"), "hello");
        assert_eq!(read_file(&root, "src/nope.txt"), "Error: File not found: src/nope.txt");
        assert!(write_file(&root, "  ", "x").starts_with("Error: write_file requires"));
        assert!(write_file(&root, "src", "x").starts_with("Error: Path is a directory"));
        assert_eq!(list_dir(&root, "missing"), "Error: Directory not found: missing");
    }

    #[test]
    fn a_listing_puts_directories_first() {
        let root = scratch("list");
        std::fs::create_dir_all(root.join("zdir")).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        assert_eq!(list_dir(&root, "."), "zdir/\na.txt");
        assert_eq!(list_dir(&root, "zdir"), "(empty directory)");
    }

    #[test]
    fn search_finds_lines_and_reports_a_miss_as_a_python_repr() {
        let root = scratch("search");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join(".venv")).unwrap();
        std::fs::write(root.join("src/app.rs"), "fn main() {}\nlet Thread = 1;\n").unwrap();
        std::fs::write(root.join("node_modules/pkg/i.js"), "thread\n").unwrap();
        std::fs::write(root.join(".venv/x.py"), "thread\n").unwrap();

        let out = search(&root, "thread");
        assert!(out.contains("src/app.rs:2: let Thread = 1;"), "{out}");
        assert!(!out.contains("node_modules"), "vendored code buries the real hits: {out}");
        assert!(!out.contains(".venv"), "dot-directories are skipped by rule: {out}");

        assert_eq!(search(&root, "nowhere"), "no matches for 'nowhere'");
        // `repr` switches to double quotes rather than escaping the apostrophe.
        assert_eq!(search(&root, "it's"), "no matches for \"it's\"");
        assert!(search(&root, "  ").starts_with("Error: search requires"));
    }

    #[test]
    fn repo_map_lists_top_level_definitions_only() {
        let root = scratch("map");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/a.rs"),
            "pub fn open(p: &Path) {}\nstruct Cfg;\nimpl Cfg {\n    fn hidden() {}\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("b.py"), "import os\nclass Store:\n    def save(self): ...\n")
            .unwrap();
        std::fs::write(root.join("c.ts"), "export function used() {}\nfunction private_() {}\n")
            .unwrap();

        let out = repo_map(&root);
        assert!(out.starts_with("definitions by file:\n"), "{out}");
        assert!(out.contains("src/a.rs: open, Cfg"), "modifiers stripped, impl bodies not: {out}");
        assert!(out.contains("b.py: Store"), "an indented def is detail: {out}");
        assert!(out.contains("c.ts: used"), "JS/TS lists exports only: {out}");
        assert!(repo_map(&scratch("map-empty")).starts_with("no definitions found"));
    }

    #[tokio::test]
    async fn commands_are_off_until_the_session_enables_them() {
        let root = scratch("cmd");
        assert!(run_command(&root, "echo hi", false)
            .await
            .starts_with("Error: command execution is disabled"));
        assert_eq!(
            run_command(&root, "   ", true).await,
            "Error: run_command requires a non-empty command"
        );
        // The exit code prefix is what the model reads, so it is pinned.
        let out = run_command(&root, "echo hi", true).await;
        assert!(out.starts_with("[exit code 0]\nhi"), "{out}");
    }

    #[tokio::test]
    async fn an_unknown_tool_is_a_result_not_a_failure() {
        let root = scratch("unknown");
        assert_eq!(
            execute_local(&root, "nonesuch", &serde_json::json!({}), true).await,
            "Error: unknown tool 'nonesuch'."
        );
    }

    #[test]
    fn the_delegated_client_is_matched_case_insensitively() {
        assert!(is_portal_desktop_client(Some("portal-desktop")));
        assert!(is_portal_desktop_client(Some("  Portal-Desktop ")));
        assert!(!is_portal_desktop_client(Some("web")));
        assert!(!is_portal_desktop_client(None));
    }
}
