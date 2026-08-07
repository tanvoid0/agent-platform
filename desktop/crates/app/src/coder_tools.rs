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
        "list_dir" => list_dir(root, &arg("path")),
        "search" => search(root, &arg("query")),
        "repo_map" => repo_map(root),
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
}
