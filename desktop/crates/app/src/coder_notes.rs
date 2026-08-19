//! Workspace notes — what the Coder agent already worked out about a folder.
//!
//! The problem: every session starts blind. The agent lists the root, reads the
//! same four files to re-derive where things live and how the tests run, and
//! only then starts on the task it was given — paying for that orientation
//! again on every new thread, against a context window that has to hold the
//! actual work too.
//!
//! So it writes what it learned down. `.agent/notes.md` in the workspace
//! itself, loaded into the system prompt of every turn, maintained by the agent
//! with the tools it already has (`read_file` / `write_file`) rather than with
//! tools of its own.
//!
//! Three consequences of putting it *in the workspace* rather than beside
//! `settings.json`:
//!
//! * It travels with the project — another checkout, or a colleague, arrives
//!   already oriented.
//! * The user can read, edit, commit or `.gitignore` it without the app's help,
//!   which is the only reason to trust a memory you cannot see.
//! * There is no key to manage. A store keyed by workspace path goes stale the
//!   first time the folder is renamed.
//!
//! It rides in `mode_instruction`, which the server already merges into the
//! system prompt (`_compose_system_prompt`) — so this half needed no server
//! change at all.

use std::path::{Path, PathBuf};

/// Where the notes live, relative to the workspace root.
pub const REL_PATH: &str = ".agent/notes.md";

/// Most of the file that is carried into a turn. Past this the tail is dropped,
/// and the block says so out loud — an agent whose notes silently stop halfway
/// has no way to know it should be trimming them.
const MAX_NOTES: usize = 1600;

/// Hard ceiling on the whole block. `mode_instruction` is `max_length=4096`
/// server-side, and a longer one is a 422 — a big notes file must cost the
/// agent detail, never the whole turn.
///
/// Well short of the 4096 on purpose: [`crate::coder::mode_instruction`] puts
/// `AGENTS.md` and the plan-gate ask in the same field. The notes are the piece
/// that grows without anyone deciding to grow it, so they get the smaller half.
const MAX_BLOCK: usize = 2400;

/// The project's own instructions to a coding agent — the converging standard
/// (Codex, Junie, Amp and Copilot all read this file). Ours to *read*, never to
/// write: the agent's own memory is `.agent/notes.md`, and a tool that edits the
/// file the humans instruct it with is one that can instruct itself.
pub const AGENTS_PATH: &str = "AGENTS.md";

/// Cap on the `AGENTS.md` block — smaller than the file often is, and said out
/// loud when it bites. The alternative is the notes and the turn's own ask
/// losing room to a file nobody thought of as a prompt budget.
const MAX_AGENTS: usize = 1200;

/// Introduces that block. Its own constant so the wording is not at the mercy
/// of a line continuation.
const AGENTS_HEADER: &str = "\n\n---\n\nThis project's own instructions to a coding agent, from its `AGENTS.md`. They are the user's rules for this repo and they win over your habits:\n\n";

/// `AGENTS.md` from the workspace root, for the system prompt. `None` when the
/// file is absent or says nothing, which is the common case and must cost the
/// prompt nothing.
pub fn agents_block(root: &Path) -> Option<String> {
    let file = std::fs::read_to_string(root.join(AGENTS_PATH)).ok()?;
    let text = file.trim();
    if text.is_empty() {
        return None;
    }
    let capped: String = text.chars().take(MAX_AGENTS).collect();
    let truncated = capped.len() < text.len();
    let mut out = String::from(AGENTS_HEADER);
    out.push_str(&capped);
    if truncated {
        out.push_str("\n...[truncated — only the first part of AGENTS.md was loaded.]");
    }
    Some(out)
}

/// `Path::join` takes the forward slash on Windows too, so the constant is
/// spelled once and the prose in [`block`] can quote the same string.
fn path(root: &Path) -> PathBuf {
    root.join(REL_PATH)
}

/// The system-prompt block for this workspace.
///
/// Always returns something. The "no notes yet, here is how to start them" half
/// is what gets the first file written at all: an agent that is never told the
/// mechanism exists never uses it.
pub fn block(root: &Path) -> String {
    let file = std::fs::read_to_string(path(root)).unwrap_or_default();
    let notes = file.trim();
    let mut out = String::new();
    if notes.is_empty() {
        out.push_str(
            "This workspace has no notes yet. `.agent/notes.md` is loaded into every turn \
             automatically, so whatever you record there you never have to work out again. \
             Start it as soon as you know something durable.",
        );
    } else {
        out.push_str(
            "What you worked out about this workspace in earlier sessions, from \
             `.agent/notes.md`. It is already loaded — do not read that file again:\n\n",
        );
        let capped: String = notes.chars().take(MAX_NOTES).collect();
        let truncated = capped.len() < notes.len();
        out.push_str(&capped);
        if truncated {
            out.push_str("\n...[truncated — the notes are too long. Shorten them.]");
        }
    }
    out.push_str(RULES);
    // Belt and braces: the arithmetic above already fits, but the failure this
    // guards is the turn not sending at all.
    out.chars().take(MAX_BLOCK).collect()
}

const RULES: &str = "\n\nKeeping them current is part of the job: when you work out something \
durable about this workspace — where a subsystem lives, how to build, test or run it, a \
convention it follows, a gotcha that cost you a wrong turn — read `.agent/notes.md`, add a \
line, and write the whole updated file back. Durable facts about the code only: no task \
history, no narration, nothing a single search would answer in seconds. Keep it under 60 \
lines, and delete anything you find to be out of date.";

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coder-notes-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, text: &str) {
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(path(root), text).unwrap();
    }

    #[test]
    fn an_empty_workspace_is_told_how_to_start_notes() {
        let root = scratch("empty");
        let block = block(&root);
        assert!(block.contains("no notes yet"));
        assert!(block.contains(REL_PATH), "the agent has to be told the path to write");

        // A file that exists but says nothing is the same as no file: an agent
        // told "here is what you know" followed by nothing learns nothing.
        write(&root, "   \n\n");
        assert!(block.contains("no notes yet"));
    }

    #[test]
    fn notes_are_carried_in_and_the_agent_is_told_not_to_re_read_them() {
        let root = scratch("carried");
        write(&root, "- Server lives in app/, desktop in desktop/crates.\n");
        let block = block(&root);
        assert!(block.contains("- Server lives in app/, desktop in desktop/crates."));
        assert!(block.contains("do not read that file again"));
        assert!(block.contains("write the whole updated file back"));
    }

    /// The failure this exists to prevent is a 422 on `mode_instruction`, which
    /// is not a degraded turn — it is no turn at all.
    /// The project's rules are read, never written, and only carried when they
    /// exist — the common case is no file at all and no cost.
    #[test]
    fn agents_md_is_carried_when_the_project_has_one() {
        let root = scratch("agents");
        assert!(agents_block(&root).is_none(), "no file is not an empty block");
        std::fs::write(root.join(AGENTS_PATH), "  \n ").unwrap();
        assert!(agents_block(&root).is_none(), "a file that says nothing says nothing");

        std::fs::write(root.join(AGENTS_PATH), "- Never edit generated files.\n").unwrap();
        let block = agents_block(&root).unwrap();
        assert!(block.contains("- Never edit generated files."));
        assert!(block.contains("win over your habits"), "a rule with no authority is a hint");

        std::fs::write(root.join(AGENTS_PATH), "x".repeat(50_000)).unwrap();
        let block = agents_block(&root).unwrap();
        assert!(block.chars().count() < MAX_AGENTS + 400, "{}", block.chars().count());
        assert!(block.contains("truncated"));
    }

    #[test]
    fn an_oversized_notes_file_costs_detail_not_the_turn() {
        let root = scratch("oversized");
        write(&root, &"x".repeat(50_000));
        let block = block(&root);
        assert!(block.chars().count() <= MAX_BLOCK);
        assert!(block.contains("the notes are too long"), "the agent is told to trim: {block}");
    }
}
