//! The workspace as a tree, and one file at a time as text.
//!
//! Read-only, deliberately. Step 4 of the hearth port is "editor + file tree",
//! and the editor is the half worth refusing: Monaco cannot be rebuilt in iced,
//! so what an editor here would be is a worse version of the one the user
//! already has open — while the thing they actually need mid-turn is to *see*
//! what the agent just wrote, without alt-tabbing away from the transcript.
//! That is this module. If editing turns out to be the ask, `iced::text_editor`
//! is one widget away and nothing here has to change.
//!
//! The tree is **flattened on demand rather than cached**: [`flatten`] walks
//! only the directories the user has opened, and re-walks them on every change.
//! A cached tree would need invalidating every time a turn writes a file, and a
//! stale file tree is worse than a slow one — this one is neither, because the
//! walk is bounded by what is on screen.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Biggest file the viewer will read. Past this it is generated, minified or a
/// data dump, and the point of the pane is reading.
const MAX_VIEW_BYTES: usize = 512 * 1024;

/// Entries per directory. A folder past this is `node_modules` by another name.
const MAX_ENTRIES: usize = 1000;

/// One row of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// File name alone — the row is indented by [`Entry::depth`], so the rest
    /// of the path is on screen already.
    pub name: String,
    pub is_dir: bool,
    /// How far below the workspace root, for the indent.
    pub depth: usize,
}

/// The rows to draw: the root's children, plus the children of every expanded
/// directory, depth-first in the order they are shown.
pub fn flatten(root: &Path, expanded: &BTreeSet<PathBuf>) -> Vec<Entry> {
    let mut out = Vec::new();
    walk(root, expanded, 0, &mut out);
    out
}

fn walk(dir: &Path, expanded: &BTreeSet<PathBuf>, depth: usize, out: &mut Vec<Entry>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<(bool, String, PathBuf)> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let is_dir = e.file_type().ok()?.is_dir();
            let name = e.file_name().to_string_lossy().into_owned();
            // Build output and tooling state, the same set the agent's own
            // `search` skips — a tree whose first screen is `node_modules` is
            // one nobody scrolls past.
            if is_dir && (crate::coder_tools::SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.'))
            {
                return None;
            }
            Some((is_dir, name, e.path()))
        })
        .take(MAX_ENTRIES)
        .collect();
    // Directories first, then case-insensitive by name — the order `list_dir`
    // hands the model, so the tree and the transcript agree.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));

    for (is_dir, name, path) in entries {
        let expand = is_dir && expanded.contains(&path);
        out.push(Entry { path: path.clone(), name, is_dir, depth });
        if expand {
            walk(&path, expanded, depth + 1, out);
        }
    }
}

/// One file as text, or why it cannot be shown.
///
/// Lossy rather than a UTF-8 error, matching `coder_tools::read_file`: a stray
/// byte in an otherwise readable file should cost one character, not the view.
/// A file that is *mostly* those bytes is a binary, and says so instead.
pub fn read_capped(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let total = data.len();
    let head = &data[..total.min(MAX_VIEW_BYTES)];
    // A NUL in the first block is the binary sniffer every editor uses, and it
    // is cheaper and more honest than a decode that "succeeds" into mojibake.
    if head.contains(&0) {
        return Err("this looks like a binary file".to_string());
    }
    let mut text = String::from_utf8_lossy(head).into_owned();
    if total > MAX_VIEW_BYTES {
        text.push_str(&format!("\n…[showing the first {MAX_VIEW_BYTES} of {total} bytes]"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coder-files-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(|e| format!("{}{}", "  ".repeat(e.depth), e.name)).collect()
    }

    #[test]
    fn only_opened_directories_are_walked() {
        let root = scratch("tree");
        std::fs::create_dir_all(root.join("src").join("deep")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(root.join("src").join("app.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("src").join("deep").join("mod.rs"), "").unwrap();
        std::fs::write(root.join("README.md"), "hi").unwrap();

        // Closed: one row per top-level entry, and none for what the agent
        // never reads either.
        let shut = flatten(&root, &BTreeSet::new());
        assert_eq!(names(&shut), vec!["src", "README.md"], "dirs first, build output skipped");

        // Opened one level: its children appear indented, its grandchildren
        // do not.
        let one = BTreeSet::from([root.join("src")]);
        assert_eq!(names(&flatten(&root, &one)), vec!["src", "  deep", "  app.rs", "README.md"]);

        let both = BTreeSet::from([root.join("src"), root.join("src").join("deep")]);
        assert_eq!(
            names(&flatten(&root, &both)),
            vec!["src", "  deep", "    mod.rs", "  app.rs", "README.md"]
        );
    }

    #[test]
    fn a_file_reads_back_and_a_binary_says_so() {
        let root = scratch("view");
        std::fs::write(root.join("a.txt"), "hello\nthere\n").unwrap();
        assert_eq!(read_capped(&root.join("a.txt")).unwrap(), "hello\nthere\n");

        std::fs::write(root.join("logo.png"), b"\x89PNG\r\n\x1a\n\x00\x00").unwrap();
        assert!(read_capped(&root.join("logo.png")).unwrap_err().contains("binary"));

        assert!(read_capped(&root.join("nope.txt")).is_err());

        let big = "x".repeat(MAX_VIEW_BYTES * 2);
        std::fs::write(root.join("big.txt"), &big).unwrap();
        let shown = read_capped(&root.join("big.txt")).unwrap();
        assert!(shown.ends_with("bytes]"), "a truncated view says it was truncated");
        assert!(shown.contains(&format!("of {} bytes", big.len())), "…and how much it left");
        assert!(shown.len() < big.len());
    }
}
