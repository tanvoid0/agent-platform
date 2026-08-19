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

use std::path::{Path, PathBuf};

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

/// Files listed for one checkpoint. The baseline holds the entire project, so
/// without a cap the panel would try to draw a row per file in the repo.
const MAX_CHANGED: usize = 200;

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

/// The user's *own* git, run in `cwd` — not the checkpoint repo [`git`] drives.
///
/// Worktrees are the one thing here that touches the project's real history, so
/// they go through their own door rather than borrowing a helper wired to
/// `--git-dir=.agent/git`.
async fn user_git(cwd: &Path, args: &[&str]) -> Result<Output, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
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

/// Where a session's isolated checkout lives, relative to the project.
const REL_WORKTREES: &str = ".agent/worktrees";

/// Whether the folder is inside a git repository of the user's own.
///
/// Synchronous and shallow on purpose: it gates a header control, so it is
/// re-read with the tree rather than awaited per frame. The checkpoint repo is
/// `.agent/git` and never a `.git`, so it cannot answer yes by accident.
pub fn is_repo(root: &Path) -> bool {
    let mut dir = Some(root);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return true;
        }
        dir = d.parent();
    }
    false
}

/// Give a session its own checkout, so what the agent does is not in the
/// user's working tree until they say so.
///
/// `--detach` rather than a branch: a session is not a branch, and one that
/// made a branch would leave it behind after the checkout was dropped.
///
/// The checkout goes under `.agent/`, which is already ours — and `.agent/` is
/// written into the repo's **`.git/info/exclude`** rather than its
/// `.gitignore`: the ignore file is the project's and belongs to whoever owns
/// the project, `info/exclude` is this clone's alone. Without it a worktree
/// shows up as untracked junk in the user's own `git status`.
pub async fn worktree_add(root: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = root.join(REL_WORKTREES).join(name);
    if dir.exists() {
        return Ok(dir);
    }
    exclude_agent_dir(root).await;
    std::fs::create_dir_all(root.join(REL_WORKTREES)).map_err(|e| e.to_string())?;
    let out = user_git(root, &["worktree", "add", "--detach", &plain(&dir)]).await?;
    if !out.ok {
        return Err(failed("worktree add", &out));
    }
    Ok(dir)
}

/// Add `.agent/` to this clone's own exclude list. Best effort: a repo whose
/// `.git` is a file (a worktree itself) or is read-only still gets a session,
/// it just also gets noise in `git status`.
async fn exclude_agent_dir(root: &Path) {
    let Ok(out) = user_git(root, &["rev-parse", "--git-common-dir"]).await else { return };
    if !out.ok {
        return;
    }
    let git_dir = PathBuf::from(out.stdout.trim());
    let git_dir = if git_dir.is_absolute() { git_dir } else { root.join(git_dir) };
    let exclude = git_dir.join("info").join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|l| l.trim() == ".agent/") {
        return;
    }
    let _ = std::fs::create_dir_all(git_dir.join("info"));
    let sep = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    let _ = std::fs::write(&exclude, format!("{current}{sep}.agent/
"));
}

/// Everything the session changed in its checkout, as a patch.
///
/// Staged first, because a file the agent *created* is untracked and a plain
/// `git diff` would not mention it — which is most of what an agent does. The
/// checkout is a scratch one, so staging in it costs nothing.
pub async fn worktree_diff(worktree: &Path) -> Result<String, String> {
    let add = user_git(worktree, &["add", "-A"]).await?;
    if !add.ok {
        return Err(failed("add", &add));
    }
    let out = user_git(worktree, &["diff", "--cached", "--binary"]).await?;
    if !out.ok {
        return Err(failed("diff", &out));
    }
    Ok(out.stdout)
}

/// Put the session's work into the real checkout.
///
/// `git apply` rather than a merge: the session is detached and uncommitted, so
/// there is no commit to merge — and apply either lands whole or refuses whole,
/// which is what a user pressing "merge back" is entitled to assume.
pub async fn worktree_merge(main: &Path, worktree: &Path) -> Result<(), String> {
    let patch = worktree_diff(worktree).await?;
    if patch.trim().is_empty() {
        return Err("that session has not changed anything yet".to_string());
    }
    let file = main.join(REL_WORKTREES).join("merge.patch");
    std::fs::create_dir_all(file.parent().unwrap_or(main)).map_err(|e| e.to_string())?;
    std::fs::write(&file, &patch).map_err(|e| e.to_string())?;
    let out = user_git(main, &["apply", "--3way", &plain(&file)]).await?;
    let _ = std::fs::remove_file(&file);
    if !out.ok {
        return Err(failed("apply", &out));
    }
    Ok(())
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

/// One file a checkpoint touched, and what it did to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// git's own letter: `A`, `M`, `D`. Rename detection is off (see
    /// [`changes`]), so those three are all that appear.
    pub status: char,
    /// Repo-relative, as git spells it — which is what [`revert_file`] takes.
    pub path: String,
}

/// What one checkpoint touched, file by file.
#[derive(Debug, Clone, PartialEq)]
pub struct Changes {
    pub files: Vec<Change>,
    /// Whether there is a commit before this one. **False for the baseline**, and
    /// that is the case this field exists for: "revert this file to before the
    /// baseline" would delete a file the user had before the agent ever ran.
    pub revertable: bool,
    /// Files beyond [`MAX_CHANGED`], so a truncated list can say so rather than
    /// read as the whole of it.
    pub hidden: usize,
}

/// The files in one checkpoint, plus whether reverting one is meaningful.
///
/// `--no-renames` on purpose: with detection on, a rename is one `R` row naming
/// two paths, and reverting "the file" would have to put the old name back as
/// well as remove the new one. Off, the same change is a `D` and an `A` — two
/// rows, each of which reverts correctly on its own.
pub async fn changes(root: &Path, sha: &str) -> Result<Changes, String> {
    let parent = format!("{sha}^");
    // `-q` so the baseline's miss is not noise on stderr.
    let revertable = git(root, &["rev-parse", "--verify", "-q", &parent]).await?.ok;
    let out = git(root, &["show", "--format=", "--name-status", "--no-renames", sha]).await?;
    if !out.ok {
        return Err(failed("show", &out));
    }
    let all: Vec<Change> = out
        .stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim_end_matches('\r').split('\t');
            let status = fields.next()?.chars().next()?;
            let path = fields.next_back()?.trim();
            (!path.is_empty()).then(|| Change { status, path: path.to_string() })
        })
        .collect();
    let hidden = all.len().saturating_sub(MAX_CHANGED);
    Ok(Changes { files: all.into_iter().take(MAX_CHANGED).collect(), revertable, hidden })
}

/// Put **one file** back to how it was before this checkpoint.
///
/// The narrow half of [`restore`]: it touches the named path and nothing else,
/// so the rest of the turn — and everything since it — stays. There is no patch
/// matcher behind this and it needs none, because a checkpoint holds whole file
/// contents; that is also why the unit here is a file rather than a hunk.
///
/// Two cases, and the second is why this is not one `git checkout`:
///
/// * the file existed before the checkpoint → check that version out;
/// * it did not → the checkpoint *created* it, so reverting means removing it.
pub async fn revert_file(root: &Path, sha: &str, path: &str) -> Result<(), String> {
    let parent = format!("{sha}^");
    if !git(root, &["rev-parse", "--verify", "-q", &parent]).await?.ok {
        return Err("this is the baseline — there is nothing before it to go back to".into());
    }
    if git(root, &["cat-file", "-e", &format!("{parent}:{path}")]).await?.ok {
        let out = git(root, &["checkout", &parent, "--", path]).await?;
        return if out.ok { Ok(()) } else { Err(failed("checkout", &out)) };
    }
    // Created by this checkpoint. Deleted through the filesystem rather than
    // `git rm`, which would also stage the removal — the next turn's `add -A`
    // records it either way, and this keeps the index the commit's alone.
    match std::fs::remove_file(root.join(path)) {
        Ok(()) => Ok(()),
        // Already gone — a later turn deleted it, or the user did. The tree is
        // in the state the revert asked for, which is what was wanted.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete {path}: {e}")),
    }
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
    /// A worktree is only worth anything if the work lands back in the real
    /// checkout, new files included — which is most of what an agent produces,
    /// and which a plain `git diff` would not have mentioned.
    #[tokio::test]
    async fn a_session_works_in_its_own_checkout_and_merges_back() {
        let root = std::env::temp_dir().join("coder-worktree-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: Vec<&'static str>| {
            let root = root.clone();
            async move { super::user_git(&root, &args).await.unwrap() }
        };
        if !run(vec!["init", "-q"]).await.ok {
            return; // no git on this machine; the other tests already say so
        }
        let _ = run(vec!["config", "user.email", "t@example.com"]).await;
        let _ = run(vec!["config", "user.name", "t"]).await;
        std::fs::write(root.join("kept.txt"), "one
").unwrap();
        assert!(run(vec!["add", "-A"]).await.ok);
        assert!(run(vec!["commit", "-qm", "base"]).await.ok);
        assert!(super::is_repo(&root), "a folder with a .git in it is a repo");

        let wt = super::worktree_add(&root, "s1").await.expect("worktree add");
        assert!(wt.join("kept.txt").is_file(), "the checkout has the project in it");

        // What an agent does: change one file, create another.
        std::fs::write(wt.join("kept.txt"), "two
").unwrap();
        std::fs::write(wt.join("made.txt"), "new
").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("kept.txt")).unwrap(),
            "one
",
            "the project is untouched until it is merged — the whole point"
        );

        super::worktree_merge(&root, &wt).await.expect("merge back");
        // Trimmed, not exact: `git apply` runs the repo's own `core.autocrlf`, so a
        // merge back on Windows can hand the file over with CRLF endings. That is
        // git doing what it does to every commit here, not the patch losing
        // anything — and asserting on the bytes would fail on one machine and
        // pass on the next.
        assert_eq!(std::fs::read_to_string(root.join("kept.txt")).unwrap().trim(), "two");
        assert_eq!(
            std::fs::read_to_string(root.join("made.txt")).unwrap().trim(),
            "new",
            "a file the agent created has to arrive too"
        );

        // `.agent/` is this clone's business, not the project's.
        let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap_or_default();
        assert!(exclude.lines().any(|l| l.trim() == ".agent/"), "got {exclude:?}");
        assert!(
            !root.join(".gitignore").exists(),
            "the project's own ignore file is not ours to write"
        );
    }

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

    /// Reverting one file out of a turn, against real git. The three cases that
    /// matter are all here: a file the turn *changed* goes back, a file it
    /// *created* goes away, and the rest of the turn stays exactly as it was —
    /// which is the whole point of per-file over whole-checkpoint restore.
    #[tokio::test]
    async fn one_file_reverts_out_of_a_turn_and_the_rest_of_it_stays() {
        if !have_git().await {
            eprintln!("skipped: no git on PATH");
            return;
        }
        let root = scratch("revert-one");
        std::fs::write(root.join("kept.py"), "keep = 1\n").unwrap();
        std::fs::write(root.join("undone.py"), "old = 1\n").unwrap();
        ensure_repo(&root).await.unwrap();

        // One turn: edits both files and adds a third.
        std::fs::write(root.join("kept.py"), "keep = 2\n").unwrap();
        std::fs::write(root.join("undone.py"), "new = 2\n").unwrap();
        std::fs::write(root.join("added.py"), "fresh = 1\n").unwrap();
        assert!(commit_all(&root, "a turn").await.unwrap());
        let turn = list(&root).await.unwrap()[0].sha.clone();

        let changed = changes(&root, &turn).await.unwrap();
        assert!(changed.revertable, "a turn on top of the baseline has something to go back to");
        let mut listed: Vec<(char, &str)> =
            changed.files.iter().map(|c| (c.status, c.path.as_str())).collect();
        listed.sort();
        assert_eq!(listed, vec![('A', "added.py"), ('M', "kept.py"), ('M', "undone.py")]);

        // A modified file goes back to the version before the turn.
        revert_file(&root, &turn, "undone.py").await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("undone.py")).unwrap(), "old = 1\n");
        // A file the turn created has no earlier version, so reverting removes it.
        revert_file(&root, &turn, "added.py").await.unwrap();
        assert!(!root.join("added.py").exists());
        // Twice is not an error: the tree is already how the revert asked for it.
        revert_file(&root, &turn, "added.py").await.unwrap();
        // And the file nobody reverted is untouched.
        assert_eq!(std::fs::read_to_string(root.join("kept.py")).unwrap(), "keep = 2\n");

        // The baseline holds the project as it was *before* the agent ran, so
        // "revert to before it" would delete files the user already had.
        let baseline = list(&root).await.unwrap().last().unwrap().sha.clone();
        assert!(!changes(&root, &baseline).await.unwrap().revertable);
        assert!(revert_file(&root, &baseline, "kept.py").await.is_err());
        assert!(root.join("kept.py").exists(), "and it did not touch the file on its way out");
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
