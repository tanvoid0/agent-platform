//! A real terminal in the Coder screen — PTY, ANSI and all.
//!
//! Not a command runner with the escape codes stripped out. This is
//! `alacritty_terminal`'s state machine (the same one Alacritty ships) driving a
//! ConPTY on Windows and a Unix PTY elsewhere, rendered by `iced_term` into an
//! iced canvas. So colour, cursor addressing, the alternate screen, scrollback,
//! selection and mouse reporting all work, and so do the programs that need
//! them — `pytest` with colour, `git log` in its pager, a dev server that stays
//! up, anything that prompts and waits for an answer.
//!
//! Why a dependency and not a port of hearth's terminal: hearth had xterm.js
//! doing the emulation and only needed the PTY half in Rust. Take xterm.js away
//! and the missing piece is an ANSI emulator, which is not a thing worth
//! hand-rolling when the crate that does it properly targets this exact iced
//! version. `iced_term` is the widget; `alacritty_terminal` is the emulator.
//!
//! What this module adds on top is the two things the agent screen needs:
//! opening the shell **in the workspace root**, and typing into it from the app
//! ([`send_line`]) so a command the agent ran can be re-run by hand.

use std::path::Path;

/// The interactive shell to open.
///
/// PowerShell on Windows rather than `cmd`, matching
/// `assistant::run_command` — a command the agent ran and a command the user
/// re-runs must mean the same thing. `-NoLogo` because a banner in a 15-line
/// drawer is most of the drawer.
///
/// `iced_term`'s own default is `wsl.exe`, which would be a different machine
/// with different paths from the one the agent's tools write to.
fn shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            std::env::var("COMSPEC")
                .ok()
                .filter(|c| c.to_lowercase().contains("powershell"))
                .unwrap_or_else(|| "powershell.exe".to_string()),
            vec!["-NoLogo".to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()), Vec::new())
    }
}

/// One open terminal.
///
/// A newtype because [`iced_term::Terminal`] is not `Debug`, and the screen's
/// state is — the alternative is hand-writing `Debug` for a struct with thirty
/// fields so that one of them can be skipped.
pub struct Session(pub iced_term::Terminal);

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session({})", self.0.id)
    }
}

/// Open a shell in `root`.
///
/// `id` must differ from every terminal opened before it in this run: it keys
/// the widget's event subscription, so a reused id would leave the new PTY
/// wired to the old subscription.
pub fn open(id: u64, root: &Path) -> Result<Session, String> {
    let (program, args) = shell();
    let settings = iced_term::settings::Settings {
        backend: iced_term::settings::BackendSettings {
            program,
            args,
            working_directory: Some(root.to_path_buf()),
            ..Default::default()
        },
        ..Default::default()
    };
    iced_term::Terminal::new(id, settings)
        .map(Session)
        .map_err(|e| format!("could not open a terminal: {e}"))
}

/// What an agent's command in the drawer produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub text: String,
    /// The exit code as the shell reported it. `0` is success on both
    /// platforms; a shell that could not report a number lands on `1`.
    pub code: i32,
}

/// Wrap `command` so the shell brackets its output with markers we can find
/// again — the agent's command runs in the *user's* terminal, so what came out
/// of it has to be told apart from whatever else is on that screen.
///
/// The exit status is reported as two tokens rather than one because the two
/// shells disagree about what an exit code is. `sh` has `$?` and it covers
/// everything. PowerShell has `$?` for "did the last thing work" and
/// `$LASTEXITCODE` for "what number did the last *native exe* return", and a
/// cmdlet sets only the first — so both are sent and the reader prefers the
/// number when there is one.
pub fn wrap(mark: &str, command: &str) -> String {
    if cfg!(windows) {
        format!(
            "Write-Host '{BEGIN}{mark}'; {command}; Write-Host \"{END}{mark} $(if($?){{0}}else{{1}}) $LASTEXITCODE\""
        )
    } else {
        format!(
            "printf '%s\n' '{BEGIN}{mark}'; {command}; printf '%s %s\n' '{END}{mark}' \"$?\""
        )
    }
}

const BEGIN: &str = "@@AGPRUN-BEGIN:";
const END: &str = "@@AGPRUN-END:";

/// Read one command's output back out of what the terminal shows.
///
/// `None` while the closing marker has not appeared — the command is still
/// running, or the shell is waiting for the user to answer it.
///
/// Two things make this safe against the shell's own echo of the line that was
/// typed. The markers must be at the **start** of a row, and the echo is
/// preceded by a prompt; and the closing one must be followed by a **number**,
/// where the echo carries `$(if($?)…)` or `%s` literally. Without the second
/// test a command wrapping onto the next row could put the marker at a row
/// start and end the command before it began.
pub fn scrape(lines: &[String], mark: &str) -> Option<Outcome> {
    let (begin_mark, end_mark) = (format!("{BEGIN}{mark}"), format!("{END}{mark}"));
    let (end, status) = lines.iter().enumerate().rev().find_map(|(i, line)| {
        let rest = line.strip_prefix(&end_mark)?;
        let mut tokens = rest.split_whitespace();
        // The first token is the shell's yes/no, the second the native exit
        // code when there was a native process to have one.
        let ok: i32 = tokens.next()?.parse().ok()?;
        let native: Option<i32> = tokens.next().and_then(|t| t.parse().ok());
        Some((i, native.filter(|_| ok != 0).unwrap_or(ok)))
    })?;
    // The *last* opening marker before it: the echoed command line carries one
    // too, and it is written before the shell ever runs the line.
    let begin = lines[..end].iter().rposition(|line| line.starts_with(&begin_mark))?;
    let text = lines[begin + 1..end].join("
").trim().to_string();
    Some(Outcome { text, code: status })
}

/// Everything the terminal is showing, scrollback first.
///
/// Needs `Terminal::text()`, which upstream `iced_term` does not have — see the
/// `[patch.crates-io]` block in `desktop/Cargo.toml`.
pub fn text(session: &Session) -> Vec<String> {
    session.0.text()
}

/// Type a line into the shell and press return.
///
/// `\r`, not `\n`: a PTY carries what a keyboard sends, and return is carriage
/// return. `\n` reaches the shell as a line feed and sits there unexecuted.
pub fn send_line(session: &mut Session, line: &str) {
    let mut bytes = line.trim_end().as_bytes().to_vec();
    bytes.push(b'\r');
    let _ = session
        .0
        .handle(iced_term::Command::ProxyToBackend(iced_term::BackendCommand::Write(bytes)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line the shell echoes back carries both markers, and it is on screen
    /// before the command has run — so a reader that trusts the first marker it
    /// sees reports an empty result for a command that never started.
    #[test]
    fn the_shells_echo_of_the_command_is_not_mistaken_for_its_output() {
        let mark = "abc";
        let lines: Vec<String> = vec![
            format!("PS D:/work> {}", wrap(mark, "cargo test")),
            format!("{BEGIN}{mark}"),
            "running 2 tests".into(),
            "test result: ok".into(),
            format!("{END}{mark} 0 0"),
            "PS D:/work>".into(),
        ];
        let out = scrape(&lines, mark).expect("the closing marker is there");
        assert_eq!(out.text, "running 2 tests
test result: ok");
        assert_eq!(out.code, 0);
    }

    /// A wrapped row can put the closing marker at the start of a line without
    /// the command having ended. The number after it is what tells them apart.
    #[test]
    fn a_marker_with_no_exit_code_after_it_is_not_the_end_of_anything() {
        let mark = "abc";
        let running: Vec<String> = vec![
            format!("{END}{mark} $(if($?){{0}}else{{1}}) $LASTEXITCODE\""),
            format!("{BEGIN}{mark}"),
            "compiling…".into(),
        ];
        assert_eq!(scrape(&running, "abc"), None, "still running");

        // And nothing at all until the shell has said so.
        assert_eq!(scrape(&["idle".to_string()], mark), None);
    }

    /// A failing command has to come back as failing. PowerShell reports the
    /// native code beside its own yes/no, and the number is the better answer.
    #[test]
    fn a_failure_keeps_the_number_the_process_actually_returned() {
        let mark = "z9";
        let lines: Vec<String> = vec![
            format!("{BEGIN}{mark}"),
            "error: test failed".into(),
            format!("{END}{mark} 1 101"),
        ];
        assert_eq!(scrape(&lines, mark).unwrap().code, 101);

        // `sh` sends one token, and a cmdlet failure sends a yes/no with no
        // number behind it — both mean "failed" without a nicer answer.
        let unix: Vec<String> = vec![format!("{BEGIN}{mark}"), format!("{END}{mark} 2")];
        assert_eq!(scrape(&unix, mark).unwrap().code, 2);
    }

    /// The shell has to be a real program on this machine, and it has to be the
    /// same one the agent's own `run_command` uses — a `cd` that works in one
    /// and not the other is the kind of difference nobody debugs twice.
    #[test]
    fn the_shell_is_this_platforms_and_not_the_crates_wsl_default() {
        let (program, args) = shell();
        assert!(!program.is_empty());
        #[cfg(windows)]
        {
            assert!(program.to_lowercase().contains("powershell"), "{program}");
            assert!(!program.to_lowercase().contains("wsl"), "wsl is a different machine");
            assert_eq!(args, vec!["-NoLogo"]);
        }
        #[cfg(not(windows))]
        assert!(program.starts_with('/'), "{program}");
    }
}
