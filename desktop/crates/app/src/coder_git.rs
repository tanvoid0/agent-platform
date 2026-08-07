//! Checkpoints — a git repo of the agent's own, over the user's files.
//!
//! Ported from hearth, and the trick is the whole feature: a **separate git dir
//! with the workspace as its work tree**
//! (`git --git-dir=<root>/.agent/git --work-tree=<root>`). So every turn that
//! changes a file becomes a commit that can be read back and rolled back,
//! without ever touching the user's own `.git` — no index of theirs is stirred,
//! no branch of theirs moves, and a workspace that is not a git repo at all
//! gets checkpoints anyway.
//!
//! This is what makes "show me what that turn did" and "put it back" two git
//! calls instead of a snapshot store nobody wants to write.
//!
//! Two rules the callers depend on:
//!
//! * **A checkpoint failure must never break a turn.** git may not be installed.
//!   Every function returns its error as a string for the screen to show in the
//!   timeline, and the turn carries on regardless — the agent's work is the
//!   point, the history of it is not.
//! * **Arguments are a list, never a shell string.** They are ours, not the
//!   model's, but a commit message *is* the user's prompt, and it goes in as one
//!   argument that nothing parses.

use std::path::Path;

/// The checkpoint repo, relative to the workspace root. Beside
/// [`crate::coder_notes::REL_PATH`], because both are the agent's own state
/// living in the user's project where they can see it.
pub const REL_GIT_DIR: &str = ".agent/git";

/// These commits are the agent's, not the user's — so they do not inherit a
/// global identity, and they do not fail on a machine that has never set one.
const IDENTITY: [&str; 4] =
    ["-c", "user.name=Coder", "-c", "user.email=coder@localhost"];

/// Checkpoints answer "what did the agent change", so directories it never
/// writes are noise — and `add -A` across `node_modules` is slow enough to feel.
/// The project's own `.gitignore` is honoured on top of this, since it sits in
/// the work tree. This goes in the repo's `info/exclude` rather than in a
/// `.gitignore`: a file in the user's project is a change they did not ask for.
const EXCLUDE: &str = "node_modules/\n.agent/\ndist/\nbuild/\n.next/\ntarget/\n";

/// Commits shown in the timeline. Older ones stay in the repo and stay
/// restorable by hand; the panel is a timeline, not an archive browser.
const MAX_LISTED: usize = 50;

/// Longest commit subject. A prompt can be a page long, and `git log --format=%s`
/// would put all of it on one row.
const MAX_SUBJECT: usize = 120;

/// One commit in the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    pub sha: String,
    /// The prompt that produced the turn.
    pub message: String,
    /// Relative age, as git renders it ("2 minutes ago").
    pub when: String,
}

impl Checkpoint {
    /// The sha as it is shown — long enough to paste into a real `git` command.
    pub fn short(&self) -> &str {
        &self.sha[..self.sha.len().min(8)]
    }
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// A path as git will accept it.
///
/// `std::fs::canonicalize` returns Windows *verbatim* paths (`\\?\C:\work`), and
/// git does not understand them: it answers `fatal: not a git repository` for a
/// directory that is right there. A root can arrive canonicalized from anywhere
/// — a thread's stored `workspace_root`, a resolved symlink — so it is stripped
/// here rather than trusted not to happen. `\\?\UNC\server\share` is the other
/// spelling, and unwraps to `\\server\share`.
fn plain(path: &Path) -> String {
    let s = path.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => match rest.strip_prefix("UNC\\") {
            Some(share) => format!(r"\\{share}"),
            None => rest.to_string(),
        },
        None => s,
    }
}

/// Run one git command against the checkpoint repo.
async fn git(root: &Path, args: &[&str]) -> Result<Output, String> {
    let git_dir = root.join(REL_GIT_DIR);
    // `git init` will not create its own parent. Doing it here rather than in
    // `ensure_repo` means no caller has to remember to.
    std::fs::create_dir_all(&git_dir).map_err(|e| e.to_string())?;

    let mut cmd = tokio::process::Command::new("git");
    cmd.arg(format!("--git-dir={}", plain(&git_dir)))
        .arg(format!("--work-tree={}", plain(root)))
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // CREATE_NO_WINDOW — a console flashing up on every turn's commit.
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);

    let out = cmd
        .output()
        .await
        .map_err(|e| format!("could not run git ({e}) — is it installed and on PATH?"))?;
    Ok(Output {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn failed(what: &str, out: &Output) -> String {
    let detail = out.stderr.trim();
    if detail.is_empty() { format!("git {what} failed") } else { format!("git {what}: {detail}") }
}

/// Create the checkpoint repo on first use, with a baseline commit.
///
/// The baseline is what makes the *first* turn's diff show that turn's changes
/// rather than the entire project. It runs before a turn rather than when a
/// folder is opened, so a folder the agent is never asked to touch never gets a
/// repo written into it.
pub async fn ensure_repo(root: &Path) -> Result<(), String> {
    if git(root, &["rev-parse", "HEAD"]).await?.ok {
        return Ok(());
    }
    let init = git(root, &["init"]).await?;
    if !init.ok {
        return Err(failed("init", &init));
    }
    let exclude = root.join(REL_GIT_DIR).join("info");
    std::fs::create_dir_all(&exclude).map_err(|e| e.to_string())?;
    std::fs::write(exclude.join("exclude"), EXCLUDE).map_err(|e| e.to_string())?;
    // Without this, restoring a checkpoint on Windows rewrites every line
    // ending in the project — a diff against the user's own git, in files git
    // was never asked to touch.
    let _ = git(root, &["config", "core.autocrlf", "false"]).await?;
    commit_all(root, "baseline").await.map(|_| ())
}

/// The commit subject for a turn: its first line, capped.
fn subject(message: &str) -> String {
    let line = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let capped: String = line.chars().take(MAX_SUBJECT).collect();
    if capped.is_empty() { "agent turn".to_string() } else { capped }
}

/// Commit whatever the turn left on disk. `Ok(false)` means it changed nothing —
/// a checkpoint with no files in it is not worth a row.
pub async fn commit_all(root: &Path, message: &str) -> Result<bool, String> {
    let add = git(root, &["add", "-A"]).await?;
    if !add.ok {
        return Err(failed("add", &add));
    }
    // Asked before committing, because `git commit` with nothing staged exits
    // non-zero — the same signal as a real failure.
    if git(root, &["status", "--porcelain"]).await?.stdout.trim().is_empty() {
        return Ok(false);
    }
    let subject = subject(message);
    let mut args: Vec<&str> = IDENTITY.to_vec();
    args.extend_from_slice(&["commit", "-m", &subject]);
    let out = git(root, &args).await?;
    if !out.ok {
        return Err(failed("commit", &out));
    }
    Ok(true)
}

/// Every checkpoint, newest first.
///
/// NUL-separated because the message is the user's prompt and can contain
/// anything, including whatever separator a friendlier format would have used.
pub async fn list(root: &Path) -> Result<Vec<Checkpoint>, String> {
    let max = format!("--max-count={MAX_LISTED}");
    let out = git(root, &["log", &max, "--format=%H%x00%s%x00%cr"]).await?;
    if !out.ok {
        // No repo yet: the agent has never run here. Not an error to show.
        return Ok(Vec::new());
    }
    Ok(out
        .stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim_end_matches('\r').split('\0');
            Some(Checkpoint {
                sha: fields.next()?.to_string(),
                message: fields.next()?.to_string(),
                when: fields.next()?.to_string(),
            })
        })
        .filter(|c| !c.sha.is_empty())
        .collect())
}

/// What one checkpoint changed: a summary, then the patch.
///
/// `git show` rather than `diff <sha>~ <sha>` because it handles the root
/// commit, which has no parent to diff against — and the root commit here is
/// the baseline, the one a user is most likely to click first.
pub async fn diff(root: &Path, sha: &str) -> Result<String, String> {
    let out = git(root, &["show", "--format=", "--stat", "--patch", "-U3", sha]).await?;
    if !out.ok {
        return Err(failed("show", &out));
    }
    let text = out.stdout.trim_end().to_string();
    Ok(if text.is_empty() { "(this checkpoint changed nothing)".to_string() } else { text })
}

/// Put the work tree back to how it looked at this checkpoint.
///
/// Destructive by design, and only to what git tracks: untracked files (build
/// output, `node_modules`, anything `info/exclude` lists) are left alone. Edits
/// made since the checkpoint — the user's own, in their editor — are not, which
/// is why the caller asks twice.
pub async fn restore(root: &Path, sha: &str) -> Result<(), String> {
    let out = git(root, &["reset", "--hard", sha]).await?;
    if out.ok {
        Ok(())
    } else {
        Err(failed("reset", &out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coder-git-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    async fn have_git() -> bool {
        tokio::process::Command::new("git")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// The whole loop, against real git: baseline, a turn's change, its diff,
    /// and the roll back. Everything here is a two-process interaction, which
    /// is exactly the part no amount of unit testing the state machine covers.
    #[tokio::test]
    async fn a_turn_becomes_a_checkpoint_that_can_be_read_and_rolled_back() {
        if !have_git().await {
            eprintln!("skipped: no git on PATH");
            return;
        }
        let root = scratch("roundtrip");
        std::fs::write(root.join("app.py"), "print('v1')\n").unwrap();

        ensure_repo(&root).await.unwrap();
        // Second call is a no-op rather than a second baseline.
        ensure_repo(&root).await.unwrap();
        assert_eq!(list(&root).await.unwrap().len(), 1, "one baseline, not two");

        // A turn that changes nothing leaves no row behind.
        assert!(!commit_all(&root, "asked a question").await.unwrap());
        assert_eq!(list(&root).await.unwrap().len(), 1);

        std::fs::write(root.join("app.py"), "print('v2')\n").unwrap();
        assert!(commit_all(&root, "bump the version\n\nsecond line").await.unwrap());

        let checkpoints = list(&root).await.unwrap();
        assert_eq!(checkpoints.len(), 2, "newest first");
        assert_eq!(checkpoints[0].message, "bump the version", "subject is the first line");
        assert_eq!(checkpoints[1].message, "baseline");
        assert_eq!(checkpoints[0].short().len(), 8);

        let patch = diff(&root, &checkpoints[0].sha).await.unwrap();
        assert!(patch.contains("app.py"), "{patch}");
        assert!(patch.contains("-print('v1')") && patch.contains("+print('v2')"), "{patch}");
        // The baseline is the root commit — `show` has to render it without a
        // parent, which is what `diff <sha>~` could not have done.
        assert!(diff(&root, &checkpoints[1].sha).await.unwrap().contains("app.py"));

        // And back. The user's own uncommitted edit goes with it, which is what
        // the confirmation in front of this exists for.
        std::fs::write(root.join("app.py"), "print('v3')\n").unwrap();
        restore(&root, &checkpoints[1].sha).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("app.py")).unwrap(), "print('v1')\n");
    }

    /// The user's own `.git` must come out of a turn untouched — that is the
    /// entire reason for the separate git dir.
    #[tokio::test]
    async fn the_users_own_git_is_never_touched() {
        if !have_git().await {
            eprintln!("skipped: no git on PATH");
            return;
        }
        let root = scratch("theirs");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git").join("MARKER"), "theirs").unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();

        ensure_repo(&root).await.unwrap();
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        commit_all(&root, "edit").await.unwrap();

        // Their directory is exactly as it was, and ours is somewhere else.
        assert_eq!(
            std::fs::read_to_string(root.join(".git").join("MARKER")).unwrap(),
            "theirs"
        );
        assert!(!root.join(".git").join("HEAD").exists(), "no repo was initialised in theirs");
        assert!(root.join(REL_GIT_DIR).join("HEAD").exists());
    }

    /// The failure this fixes is invisible from the path itself: the directory
    /// exists, and git says it is not a repository.
    #[test]
    fn a_canonicalized_windows_path_is_handed_to_git_the_way_git_spells_it() {
        assert_eq!(plain(Path::new(r"\\?\C:\work\demo")), r"C:\work\demo");
        assert_eq!(plain(Path::new(r"\\?\UNC\box\share\demo")), r"\\box\share\demo");
        // Everything else, including every POSIX path, is left alone.
        assert_eq!(plain(Path::new(r"C:\work\demo")), r"C:\work\demo");
        assert_eq!(plain(Path::new("/home/t/demo")), "/home/t/demo");
    }

    #[test]
    fn a_long_prompt_does_not_become_a_long_row() {
        assert_eq!(subject("  fix the parser  \nand then some"), "fix the parser");
        assert_eq!(subject("   "), "agent turn");
        assert_eq!(subject(&"x".repeat(500)).len(), MAX_SUBJECT);
    }
}
