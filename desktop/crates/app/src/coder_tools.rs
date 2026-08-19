//! The desktop half of the Coder agent's tools.
//!
//! The agent loop itself lives on the server (`app/coder/service.py`). What runs
//! here is the *executor*: with `delegate_tools`, the server emits a `tool_call`
//! frame and parks the turn on a future, and this module produces the string
//! that answers it. So the model is wherever the proxy points, and the files it
//! reads and writes are this machine's.
//!
//! Results mirror `coder/executor.py`'s `LocalExecutor` verbatim — the same
//! wording, the same `Error: …` prefix, the same truncation notes. A model that
//! sees one shape when the platform runs locally and another when it runs
//! remotely is being asked to learn two tools.
//!
//! Failures come back as tool results rather than as errors: the model reads
//! "File not found: src/ap.rs" and tries `src/app.rs` next, where a thrown error
//! would end the turn.

use std::path::{Path, PathBuf};

/// A read handed to the model, before the server's own token budget trims it
/// further. Matches `MAX_READ_BYTES` in `coder/executor.py`.
const MAX_READ_BYTES: usize = 256 * 1024;
/// Entries per `list_dir`. A directory past this wants `search`, not a listing.
const MAX_DIR_ENTRIES: usize = 500;
/// Hard deadline on one command; past it the process is killed.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

// `search` / `repo_map` limits, matching `coder/executor.py` constant for
// constant. Past the size cap a file is a bundle, a lockfile or a data dump: a
// hit in one is never what was meant, and reading them is most of what a search
// costs.
const SEARCH_MAX_FILE_BYTES: u64 = 1_000_000;
const SEARCH_MAX_HITS: usize = 100;
/// One matching line, clipped — a minified line under the size cap would
/// otherwise be the whole result.
const SEARCH_MAX_HIT_CHARS: usize = 300;
const MAP_MAX_FILES: usize = 400;
/// Tooling state and build output. Dot-directories are skipped by rule (`.git`,
/// `.venv`, `.hearth`); dot *files* are project config someone may be after.
pub(crate) const SKIP_DIRS: [&str; 7] =
    ["node_modules", "target", "dist", "build", "__pycache__", "venv", "site-packages"];

/// The tool list this screen advertises, as `SendRequest.tools`.
///
/// Six of these mirror the server's own `TOOL_SPECS_JSON` verbatim
/// (`server/src/coder.rs`), because sending `tools` **replaces** the list for
/// the whole turn — the additions cannot travel on their own. So both copies
/// change together: a spec added server-side and not here is one this screen's
/// turns never see, even though its executor would run it.
///
/// Two are this side's own:
///
/// * `update_todos` never reaches [`execute`] at all — the checklist is screen
///   state, so `coder::update` answers that call itself.
/// * `edit_file` is implemented in *both* executors but advertised only here.
///   That is the gate: the wire contract is shared with portal_desktop, which
///   delegates and has no `edit_file` yet, so putting it in the server's default
///   list would send their models a tool their machine cannot run. Advertising
///   it from the client that has it keeps the rollout to the client that has it.
const TOOL_SPECS_JSON: &str = r#"[
  {"type": "function", "function": {"name": "read_file", "description": "Read a text file from the workspace. Path is relative to the workspace root.", "parameters": {"type": "object", "properties": {"path": {"type": "string", "description": "Relative file path, e.g. 'src/app.py'"}}, "required": ["path"]}}},
  {"type": "function", "function": {"name": "write_file", "description": "Create or overwrite a text file in the workspace. Parent directories are created automatically.", "parameters": {"type": "object", "properties": {"path": {"type": "string", "description": "Relative file path"}, "content": {"type": "string", "description": "Full new file content"}}, "required": ["path", "content"]}}},
  {"type": "function", "function": {"name": "edit_file", "description": "Change part of an existing file: replace old_text with new_text, once. Prefer this over write_file for any file you did not just create — it is cheaper and the change is easier to review. old_text must appear exactly once, so include enough surrounding lines to be unique, and copy it exactly, indentation included.", "parameters": {"type": "object", "properties": {"path": {"type": "string", "description": "Relative file path"}, "old_text": {"type": "string", "description": "The exact text to replace, copied from the file"}, "new_text": {"type": "string", "description": "What to put in its place; empty deletes it"}}, "required": ["path", "old_text", "new_text"]}}},
  {"type": "function", "function": {"name": "list_dir", "description": "List entries in a workspace directory. Directories end with '/'.", "parameters": {"type": "object", "properties": {"path": {"type": "string", "description": "Relative directory path; omit or '.' for the root"}}, "required": []}}},
  {"type": "function", "function": {"name": "search", "description": "Find which files contain a literal string, case-insensitively. Use this to locate code instead of reading files one at a time.", "parameters": {"type": "object", "properties": {"query": {"type": "string", "description": "Literal text to find, e.g. 'def send_message'"}}, "required": ["query"]}}},
  {"type": "function", "function": {"name": "repo_map", "description": "List the top-level definitions of every source file in the workspace (Python, Rust, JavaScript/TypeScript). Use this to see what exists and where a name lives before reading anything.", "parameters": {"type": "object", "properties": {}, "required": []}}},
  {"type": "function", "function": {"name": "run_command", "description": "Run a shell command in the workspace root and return stdout/stderr. Only available when command execution is enabled for the session.", "parameters": {"type": "object", "properties": {"command": {"type": "string", "description": "Shell command, e.g. 'pytest -q'"}}, "required": ["command"]}}},
  {"type": "function", "function": {"name": "update_todos", "description": "Show the user your checklist for this task. Call it once you know the steps, and again each time one is finished. Send the whole list every time — it replaces the last one. Use it for anything that takes more than one step; it is how the user follows what you are doing.", "parameters": {"type": "object", "properties": {"items": {"type": "array", "description": "The whole checklist, in order", "items": {"type": "object", "properties": {"text": {"type": "string", "description": "One step, short, e.g. 'add the migration'"}, "done": {"type": "boolean", "description": "Whether that step is finished"}}, "required": ["text", "done"]}}}, "required": ["items"]}}}
]"#;

/// The tool list for a turn, parsed once per send.
pub fn tool_specs() -> Vec<serde_json::Value> {
    serde_json::from_str(TOOL_SPECS_JSON).expect("TOOL_SPECS_JSON is valid JSON")
}

/// Resolve a model-supplied path against the workspace root, refusing anything
/// that leaves it.
///
/// Absolute paths are allowed *if they land inside*, which is what
/// `LocalExecutor._resolve` does — a model that read an absolute path out of a
/// build log will hand it back verbatim.
///
/// Canonicalizing is the whole check: it resolves `..` and follows symlinks, so
/// a link inside the root pointing at `~/.ssh` is caught by where it *lands*
/// rather than by how it is spelled. A file that does not exist yet cannot be
/// canonicalized, so the walk drops components until something does and
/// re-attaches the rest — by then `..` is already gone.
pub fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let base = std::fs::canonicalize(root)
        .map_err(|e| format!("Workspace root is unreadable: {}: {e}", root.display()))?;
    let raw = rel.trim();
    let raw = if raw.is_empty() { "." } else { raw };
    let candidate = Path::new(raw);
    let joined =
        if candidate.is_absolute() { candidate.to_path_buf() } else { base.join(candidate) };

    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe = joined;
    let resolved = loop {
        if let Ok(found) = std::fs::canonicalize(&probe) {
            break found;
        }
        match (probe.file_name().map(|n| n.to_os_string()), probe.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                probe = parent.to_path_buf();
            }
            _ => return Err(format!("Error: no such directory: {rel}")),
        }
    };
    let target = tail.iter().rev().fold(resolved, |p, name| p.join(name));

    if target.starts_with(&base) {
        Ok(target)
    } else {
        Err(format!("Error: Path escapes the workspace root and was blocked: {rel}"))
    }
}

/// Run a workspace walk on the blocking pool. A panic in there is reported as
/// the tool error it is — the agent loop is waiting for a string either way.
async fn blocking<F>(f: F) -> String
where
    F: FnOnce() -> String + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(out) => out,
        Err(e) => format!("Error: the workspace scan failed ({e})."),
    }
}

/// Run one delegated tool call and return what the model should see.
pub async fn execute(
    root: &Path,
    tool: &str,
    args: &serde_json::Value,
    allow_commands: bool,
) -> String {
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    match tool {
        "read_file" => read_file(root, &arg("path")),
        "write_file" => write_file(root, &arg("path"), &arg("content")),
        "edit_file" => edit_file(root, &arg("path"), &arg("old_text"), &arg("new_text")),
        "list_dir" => list_dir(root, &arg("path")),
        // Off the async runtime (ADR 0010). Both walk the whole workspace with
        // `read_dir` and then read every file they found — seconds of a parked
        // tokio worker on a real repo, and this runs on the app's runtime, which
        // is also what draws the UI.
        "search" => {
            let (root, query) = (root.to_path_buf(), arg("query"));
            blocking(move || search(&root, &query)).await
        }
        "repo_map" => {
            let root = root.to_path_buf();
            blocking(move || repo_map(&root)).await
        }
        "run_command" if !allow_commands => "Error: command execution is disabled for this \
             session. Ask the user to enable it (allow_commands) if a command is required."
            .to_string(),
        "run_command" => {
            crate::assistant::run_command(arg("command"), Some(root.to_path_buf()), COMMAND_TIMEOUT)
                .await
        }
        other => format!("Error: unknown tool '{other}'."),
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
    // Lossy rather than a UTF-8 error: a stray byte in an otherwise readable
    // file should cost one character, not the whole read.
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
    if path.is_dir() {
        return format!("Error: Path is a directory, not a file: {rel}");
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("Error: {e}");
        }
    }
    match std::fs::write(&path, content) {
        Ok(()) => format!("Wrote {} bytes to {rel}", content.len()),
        Err(e) => format!("Error: {e}"),
    }
}

/// Replace one exact block of text, rather than rewriting the file around it.
///
/// `write_file` stays for new files and for a rewrite that really is the whole
/// file; this is what makes a three-line change cost three lines of tokens
/// instead of a thousand, and it is what makes the checkpoint diff readable.
///
/// The match must be **unique**. An `old_text` that appears twice is a model
/// that did not read enough context, and picking one of them is how an agent
/// silently edits the wrong function.
///
/// One fallback, and only one: trailing whitespace per line is ignored, because
/// a model re-typing a block drops it constantly. Leading whitespace is not —
/// indentation is meaning in Python and YAML, and a helpful re-indent is a
/// silent corruption. Nothing is written unless exactly one place matched.
fn edit_file(root: &Path, rel: &str, old: &str, new: &str) -> String {
    if old.is_empty() {
        return "Error: edit_file requires old_text. Use write_file to create a file."
            .to_string();
    }
    let path = match resolve_in_root(root, rel) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !path.is_file() {
        return format!("Error: File not found: {rel}");
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return format!("Error: {e}"),
    };
    let updated = match replace_block(&text, old, new) {
        Ok(t) => t,
        Err(0) => {
            return format!(
                "Error: edit_file found no match in {rel}. Read the file and copy old_text \
                 exactly, including indentation. Nothing was changed."
            )
        }
        Err(n) => {
            return format!(
                "Error: old_text appears {n} times in {rel}. Include more of the surrounding \
                 lines so it matches once. Nothing was changed."
            )
        }
    };
    match std::fs::write(&path, &updated) {
        Ok(()) => format!(
            "Edited {rel}: {} lines replaced with {}.",
            old.lines().count(),
            new.lines().count()
        ),
        Err(e) => format!("Error: {e}"),
    }
}

/// The splice behind [`edit_file`]. `Err(n)` is how many places matched when
/// that was not exactly one.
///
/// The fallback works on byte ranges rather than on a rebuilt `Vec<&str>` so
/// the file's own line endings survive: rejoining `lines()` with `\n` rewrites
/// every CRLF in the file, and a one-line edit that reports as a whole-file
/// diff is worse than no edit tool at all.
fn replace_block(text: &str, old: &str, new: &str) -> Result<String, usize> {
    match text.matches(old).count() {
        1 => return Ok(text.replacen(old, new, 1)),
        0 => {}
        n => return Err(n),
    }
    let needle: Vec<&str> = old.lines().map(str::trim_end).collect();
    if needle.is_empty() {
        return Err(0);
    }
    // Start offset of every line, plus a sentinel past the end.
    let mut starts: Vec<usize> = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let content_end = |line: usize| -> usize {
        let end = starts.get(line + 1).copied().unwrap_or(text.len());
        text[..end].trim_end_matches(['\n', '\r']).len().max(starts[line])
    };
    if needle.len() > starts.len() {
        return Err(0);
    }
    let hits: Vec<usize> = (0..=starts.len() - needle.len())
        .filter(|&i| {
            (0..needle.len()).all(|k| text[starts[i + k]..content_end(i + k)].trim_end() == needle[k])
        })
        .collect();
    match hits.len() {
        1 => {
            let (from, to) = (starts[hits[0]], content_end(hits[0] + needle.len() - 1));
            Ok(format!("{}{new}{}", &text[..from], &text[to..]))
        }
        n => Err(n),
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
    // Directories first, then case-insensitive by name — the order
    // `LocalExecutor` sorts in, so a listing reads the same either side.
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

/// Every file under the root, in a stable order, skipping build output.
///
/// Stable is the point: the caps below cut a walk off part-way, so an arbitrary
/// order would return a different hundred hits each call. The order is Python's
/// `sorted(iterdir(), key=lambda c: c.name)` — case-*sensitive*, because that is
/// what `_walk_files` does and the two lists have to agree.
///
/// Walked by hand rather than shelling out to ripgrep: an app that has to work
/// offline on a machine it did not provision cannot assume the binary is there.
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

/// A walked path as the model should spell it back to `read_file`: relative to
/// the root, forward slashes on every platform.
fn rel_posix(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Read a file as text, or `None` if it is too big or is not text at all.
///
/// A failed UTF-8 decode *is* the binary sniffer, exactly as in Python — hence
/// `from_utf8` rather than the lossy read `read_file` uses, where one stray byte
/// should cost a character rather than the whole file.
fn read_text(path: &Path, max_bytes: u64) -> Option<String> {
    if path.metadata().ok()?.len() > max_bytes {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

/// Find a literal string in the workspace's file contents.
///
/// Literal and case-insensitive rather than a regex, following hearth: a model
/// writes a broken regex often enough that the failure mode becomes an error
/// loop, and it cannot tell a pattern that matched nothing from one that never
/// compiled.
fn search(root: &Path, query: &str) -> String {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return "Error: search requires a non-empty query".to_string();
    }
    let root = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) => return format!("Workspace root is unreadable: {}: {e}", root.display()),
    };
    let mut hits: Vec<String> = Vec::new();
    let mut truncated = false;
    'files: for path in walk_files(&root) {
        if hits.len() >= SEARCH_MAX_HITS {
            truncated = true;
            break;
        }
        let Some(text) = read_text(&path, SEARCH_MAX_FILE_BYTES) else { continue };
        let rel = rel_posix(&root, &path);
        for (i, line) in text.lines().enumerate() {
            if hits.len() >= SEARCH_MAX_HITS {
                truncated = true;
                break 'files;
            }
            if line.to_lowercase().contains(&needle) {
                let trimmed = line.trim();
                let shown: String = trimmed.chars().take(SEARCH_MAX_HIT_CHARS).collect();
                hits.push(format!("{rel}:{}: {shown}", i + 1));
            }
        }
    }
    if hits.is_empty() {
        return format!("no matches for '{}'", query.trim());
    }
    if truncated {
        hits.push(format!("...[truncated at {SEARCH_MAX_HITS} matches — narrow the query]"));
    }
    hits.join("\n")
}

// --- repo_map: what counts as a definition -----------------------------------
// A token walk rather than regexes, because the same walk exists in Python
// (`coder/executor.py`) and two regex dialects drifting apart is a map that says
// different things depending on where the agent is running.
//
// Only column 0 counts, in every language: a Rust `impl` block's methods and a
// Python class's methods are detail, and the map answers "where does this name
// live", not "what is in this file".

/// Extension → (keywords that declare a name, whether `export` is required).
fn map_language(ext: &str) -> Option<(&'static [&'static str], bool)> {
    const PY: &[&str] = &["def", "class"];
    const RS: &[&str] = &[
        "fn", "struct", "enum", "trait", "type", "const", "static", "mod", "macro_rules!",
    ];
    const JS: &[&str] =
        &["function", "class", "const", "let", "var", "interface", "type", "enum"];
    match ext {
        "py" | "pyi" => Some((PY, false)),
        "rs" => Some((RS, false)),
        // JS/TS: only exported names, which are the ones importers use.
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => Some((JS, true)),
        _ => None,
    }
}

/// Words allowed between the line start and the keyword. `pub(crate)` is matched
/// by prefix.
const MAP_MODIFIERS: [&str; 7] =
    ["pub", "async", "unsafe", "extern", "default", "declare", "abstract"];

/// The name a source line declares, or `None` if it declares nothing.
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
    let name: String =
        tokens.next()?.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
    (!name.is_empty()).then_some(name)
}

/// Every top-level definition in the workspace, by file — so the agent can see
/// what exists and where a name lives without reading anything.
fn repo_map(root: &Path) -> String {
    let root = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) => return format!("Workspace root is unreadable: {}: {e}", root.display()),
    };
    let mut lines: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut truncated = false;
    for path in walk_files(&root) {
        let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
        let Some((keywords, require_export)) = map_language(&ext) else { continue };
        if scanned >= MAP_MAX_FILES {
            truncated = true;
            break;
        }
        scanned += 1;
        // No size cap here: Python reads the file outright, and a source file
        // past a megabyte is generated anyway.
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
        // list_dir already says what exists, and this answers a different
        // question — where a given name lives.
        if !names.is_empty() {
            lines.push(format!("{}: {}", rel_posix(&root, &path), names.join(", ")));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway root under the OS temp dir, named per test so they can run
    /// in parallel.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coder-tools-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    /// The list is a copy of the server's, so the check that matters is that it
    /// still parses and still names every tool one of the two sides can run —
    /// a mangled edit here silently sends a turn with the wrong tools.
    #[test]
    fn the_advertised_tools_are_the_ones_something_can_run() {
        let specs = tool_specs();
        let names: Vec<String> = specs
            .iter()
            .map(|s| s["function"]["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "read_file",
                "write_file",
                "edit_file",
                "list_dir",
                "search",
                "repo_map",
                "run_command",
                // Answered by `coder::update`, not by `execute` — see the
                // constant's own note.
                "update_todos",
            ]
        );
        for spec in &specs {
            assert!(spec["function"]["parameters"].is_object(), "{spec} has no parameters");
        }
    }

    /// The four outcomes that matter, and the one that is easy to get wrong:
    /// a file with CRLF endings must come back with CRLF endings, or a one-line
    /// edit reports as a whole-file diff and the checkpoint is unreadable.
    #[test]
    fn an_edit_replaces_one_block_and_refuses_an_ambiguous_one() {
        let root = scratch("edit");
        let path = root.join("app.rs");
        std::fs::write(&path, "fn a() {\r\n    let x = 1;\r\n}\r\nfn b() {\r\n    let x = 1;\r\n}\r\n")
            .unwrap();

        // Two identical blocks: the model did not read enough, and guessing is
        // how the wrong function gets edited.
        let out = edit_file(&root, "app.rs", "    let x = 1;", "    let x = 2;");
        assert!(out.contains("appears 2 times"), "{out}");
        assert!(out.contains("Nothing was changed"), "{out}");

        // Enough context to be unique.
        let out = edit_file(&root, "app.rs", "fn b() {\r\n    let x = 1;", "fn b() {\r\n    let x = 2;");
        assert!(out.starts_with("Edited app.rs"), "{out}");
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "fn a() {\r\n    let x = 1;\r\n}\r\nfn b() {\r\n    let x = 2;\r\n}\r\n");

        // Trailing whitespace is the one thing a re-typed block loses; leading
        // whitespace is not, because indentation is meaning.
        let out = edit_file(&root, "app.rs", "fn a() {   ", "fn c() {");
        assert!(out.starts_with("Edited app.rs"), "{out}");
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("fn c() {\r\n"));
        // …and the fallback still requires the indentation to be right: a tab
        // where the file has spaces is not the same line, because a helpful
        // re-indent is a silent corruption.
        let out = edit_file(&root, "app.rs", "\tlet x = 2;\r\n}", "\tlet x = 4;\r\n}");
        assert!(out.contains("no match"), "{out}");

        assert!(edit_file(&root, "nope.rs", "a", "b").contains("File not found"));
        assert!(edit_file(&root, "app.rs", "", "b").contains("requires old_text"));
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
        // An absolute path outside is refused for the same reason, and one
        // inside is allowed — a model echoing back a path from a build log.
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

    #[test]
    fn read_and_write_round_trip_and_report_the_same_way_the_server_does() {
        let root = scratch("rw");
        assert_eq!(write_file(&root, "src/a.txt", "hello"), "Wrote 5 bytes to src/a.txt");
        assert_eq!(read_file(&root, "src/a.txt"), "hello");
        assert_eq!(read_file(&root, "src/nope.txt"), "Error: File not found: src/nope.txt");
        assert!(write_file(&root, "  ", "x").starts_with("Error: write_file requires"));
        assert!(write_file(&root, "src", "x").starts_with("Error: Path is a directory"));
    }

    #[test]
    fn a_listing_puts_directories_first() {
        let root = scratch("list");
        std::fs::create_dir_all(root.join("zdir")).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        assert_eq!(list_dir(&root, "."), "zdir/\na.txt");
        assert_eq!(list_dir(&root, "zdir"), "(empty directory)");
        assert_eq!(list_dir(&root, "missing"), "Error: Directory not found: missing");
    }

    #[test]
    fn search_finds_lines_without_the_agent_reading_the_files() {
        let root = scratch("search");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join(".venv")).unwrap();
        std::fs::write(root.join("src/app.rs"), "fn main() {}\nlet Thread = 1;\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "// thread pool\n").unwrap();
        std::fs::write(root.join("node_modules/pkg/i.js"), "thread\n").unwrap();
        std::fs::write(root.join(".venv/x.py"), "thread\n").unwrap();

        let out = search(&root, "thread");
        // Case-insensitive, 1-based line numbers, and paths spelled the way
        // `read_file` wants them back — forward slashes, even here.
        assert!(out.contains("src/app.rs:2: let Thread = 1;"), "{out}");
        assert!(out.contains("src/lib.rs:1: // thread pool"), "{out}");
        assert!(!out.contains("node_modules"), "vendored code buries the real hits: {out}");
        assert!(!out.contains(".venv"), "dot-directories are skipped by rule: {out}");

        assert_eq!(search(&root, "nowhere-at-all"), "no matches for 'nowhere-at-all'");
        assert!(search(&root, "  ").starts_with("Error: search requires"));
    }

    #[test]
    fn search_skips_binaries_and_caps_what_it_returns() {
        let root = scratch("search-caps");
        // Invalid UTF-8 is the binary sniffer, matching Python's decode failure.
        std::fs::write(root.join("blob.bin"), b"hit \xff\xfe hit").unwrap();
        std::fs::write(root.join("many.txt"), "hit\n".repeat(SEARCH_MAX_HITS + 20)).unwrap();

        let out = search(&root, "hit");
        assert!(!out.contains("blob.bin"), "undecodable means binary: {out}");
        assert_eq!(out.lines().filter(|l| l.starts_with("many.txt:")).count(), SEARCH_MAX_HITS);
        assert!(out.ends_with("matches — narrow the query]"), "{out}");
    }

    /// The map answers "where does this name live" — so a nested definition is
    /// noise and a file with none of its own is not a row at all.
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
        std::fs::write(root.join("notes.md"), "# not code\n").unwrap();

        let out = repo_map(&root);
        assert!(out.starts_with("definitions by file:\n"), "{out}");
        assert!(out.contains("src/a.rs: open, Cfg"), "modifiers stripped, impl bodies not: {out}");
        assert!(out.contains("b.py: Store"), "an indented def is detail: {out}");
        assert!(out.contains("c.ts: used"), "JS/TS lists exports only: {out}");
        assert!(!out.contains("notes.md"), "not a mapped language: {out}");

        assert!(repo_map(&scratch("map-empty")).starts_with("no definitions found"));
    }

    #[tokio::test]
    async fn commands_are_off_until_the_session_enables_them() {
        let root = scratch("cmd");
        let args = serde_json::json!({ "command": "echo hi" });
        let out = execute(&root, "run_command", &args, false).await;
        assert!(out.starts_with("Error: command execution is disabled"), "{out}");
        assert_eq!(
            execute(&root, "nonesuch", &serde_json::json!({}), true).await,
            "Error: unknown tool 'nonesuch'."
        );
    }

    /// `search` and `repo_map` are dispatched through `spawn_blocking` (ADR
    /// 0010), which is the only tool arm that hands its work to another thread.
    /// The risk is not the walk — that is unit-tested directly below — it is the
    /// wiring: a wrong closure, a dropped argument, or a join error swallowed as
    /// an empty string all still compile and all return something the model
    /// would read as "no matches".
    #[tokio::test]
    async fn the_offloaded_walks_return_what_the_direct_call_does() {
        let root = scratch("offload");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/app.rs"), "fn tidepool() {}
").unwrap();

        let hit = execute(&root, "search", &serde_json::json!({ "query": "tidepool" }), false).await;
        assert_eq!(hit, search(&root, "tidepool"), "offloaded search drifted from the direct call");
        assert!(hit.contains("src/app.rs:1"), "{hit}");

        let map = execute(&root, "repo_map", &serde_json::json!({}), false).await;
        assert_eq!(map, repo_map(&root));
        assert!(map.contains("tidepool"), "{map}");

        // A query that matches nothing still has to come back as the walk's own
        // "no matches" string, not as a swallowed join error.
        let miss = execute(&root, "search", &serde_json::json!({ "query": "zzz" }), false).await;
        assert_eq!(miss, search(&root, "zzz"));
    }
}
