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
