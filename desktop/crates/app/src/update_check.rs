//! "Is there a newer build?", and installing it.
//!
//! Two halves. The **check** asks GitHub for the newest `v*` tag and compares
//! it to this binary's own version — a button, never a poll, because this app
//! runs offline by design. The **install** fetches that release's Windows zip,
//! verifies its published SHA-256, and swaps both exes in place.
//!
//! The swap is the part with a trick in it: Windows locks a running `.exe`
//! against deletion but *allows renaming it*, so each binary is moved aside to
//! `.old` and the new one copied into the name it vacated. The running process
//! keeps executing the renamed file until [`crate::shell::spawn_replacement`]
//! starts the new one and this one exits. [`sweep_old`] clears the leftovers on
//! the next boot, when nothing holds them.
//!
//! The daemon has a self-updater of its own — `dist` generates an
//! `agent-platform-server-<target>-update` binary per platform (see
//! `dist-workspace.toml`). That one is for a daemon installed by itself; a
//! desktop install has both exes in one directory and replaces them together.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The tag series `dist` publishes under — one release carries both the daemon
/// and this app, and the two crates are versioned in lockstep, so the tag's
/// version *is* this binary's version. There was briefly a second series
/// (`desktop-v*`, from a workflow of its own); a prefix that matches nothing
/// makes this card claim "up to date" forever, which is the one answer it must
/// never give wrongly.
const TAG_PREFIX: &str = "v";
const RELEASES_API: &str =
    "https://api.github.com/repos/tanvoid0/agent-platform/releases?per_page=20";
pub const RELEASES_PAGE: &str = "https://github.com/tanvoid0/agent-platform/releases";

/// GitHub rejects a request with no `User-Agent`, with a 403 that says nothing
/// about the header.
const USER_AGENT: &str = concat!("agent-platform-desktop/", env!("CARGO_PKG_VERSION"));

#[derive(Default)]
pub struct State {
    pub checking: bool,
    /// The newest published version, once a check has come back. `None` before
    /// the first check, and when the newest release is this one.
    pub newer: Option<String>,
    pub error: Option<String>,
    /// Set by the first completed check, so the card can say "up to date"
    /// rather than staying blank.
    pub checked: bool,
    /// A download-and-swap is in flight. The server is stopped for its
    /// duration, so the card says so rather than leaving a dead sidecar
    /// unexplained.
    pub installing: bool,
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The newest `v*` release that is newer than this build, or `None`.
///
/// Blocking, because `reqwest`'s blocking client is what this crate already
/// carries and the caller runs it off the UI thread anyway.
pub fn newer_release() -> Result<Option<String>, String> {
    let response = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| "Could not reach github.com to check for updates.".to_string())?;
    if !response.status().is_success() {
        // Rate limiting is the common one and it is not the user's fault, so
        // the status is reported rather than dressed up as a failure.
        return Err(format!("GitHub answered {} for the release list.", response.status()));
    }
    let releases: Vec<Release> = response.json().map_err(|e| e.to_string())?;
    Ok(pick_newer(current(), &releases))
}

fn pick_newer(current: &str, releases: &[Release]) -> Option<String> {
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| r.tag_name.strip_prefix(TAG_PREFIX))
        .filter(|v| is_newer(v, current))
        // The API returns newest first, but it orders by publish date and a
        // backfilled tag would break that — so this picks by version.
        .max_by(|a, b| parts(a).cmp(&parts(b)))
        .map(str::to_string)
}

/// `x.y.z` only, which is what both crates in this workspace use. A version
/// this cannot parse sorts as `[0, 0, 0]` and therefore never wins, which is
/// the safe direction: a tag nobody can read must not prompt an upgrade.
fn parts(version: &str) -> [u64; 3] {
    let mut out = [0u64; 3];
    for (slot, field) in out.iter_mut().zip(version.split(['.', '-', '+'])) {
        *slot = field.parse().unwrap_or(0);
    }
    out
}

fn is_newer(candidate: &str, current: &str) -> bool {
    parts(candidate) > parts(current)
}

// ---------------------------------------------------------------------------
// Install: fetch the release zip, verify it, swap both exes
// ---------------------------------------------------------------------------

/// The two binaries a desktop install is made of. The app spawns the daemon
/// from its own directory, so replacing one without the other ships a version
/// skew across the wire contract — they move together or not at all.
const APP_EXE: &str = "agent-platform.exe";
const DAEMON_EXE: &str = "agent-platformd.exe";

/// The only target `release-desktop.yml` builds. macOS and Linux have no app
/// artifact to download at all, which is why [`install`] refuses there rather
/// than 404ing halfway.
const APP_TARGET: &str = "x86_64-pc-windows-msvc";

const DOWNLOAD_BASE: &str = "https://github.com/tanvoid0/agent-platform/releases/download";

/// Download release `version`, verify it, and swap it over this install.
///
/// The caller must have stopped the daemon first: its exe is replaced too, and
/// a running child holds the handle. Returns once the new files are in place —
/// **the process is still the old binary** and the caller relaunches
/// (`Message::RestartApp`) to pick up the new one.
///
/// Blocking, like [`newer_release`], and for the same reason: the caller runs
/// it in `spawn_blocking`.
pub fn install(version: &str) -> Result<(), String> {
    if !cfg!(windows) {
        return Err(format!(
            "Only {APP_TARGET} builds of the app are published. Update from source here."
        ));
    }
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate this binary: {e}"))?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| "This binary has no parent directory to install into.".to_string())?
        .to_path_buf();

    let stem = format!("agent-platform-desktop-{version}-{APP_TARGET}");
    let url = format!("{DOWNLOAD_BASE}/v{version}/{stem}.zip");

    // A fresh staging dir per attempt: a half-unpacked one from a failed run is
    // exactly the state that would put a truncated exe into the install.
    let staging = std::env::temp_dir().join(format!("agent-platform-update-{version}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("Could not create a staging directory: {e}"))?;

    let archive = staging.join("update.zip");
    download(&url, &archive)?;
    verify(&archive, &format!("{url}.sha256"))?;
    unpack(&archive, &staging)?;

    // Everything above is undoable by deleting a temp directory. Nothing below
    // is, so both binaries are proven present before either is touched.
    let staged = locate(&staging)?;
    swap(&staged, &install_dir)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        // Generous next to the check's ten seconds: this is tens of megabytes,
        // not a JSON list.
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let response = client()?
        .get(url)
        .send()
        .map_err(|_| "Could not reach github.com to download the update.".to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub answered {} for {url}.", response.status()));
    }
    let bytes = response.bytes().map_err(|e| format!("The download failed: {e}"))?;
    std::fs::write(dest, &bytes).map_err(|e| format!("Could not write the download: {e}"))
}

/// Check the archive against the `.sha256` published beside it.
///
/// Both come from the same host, so this is an integrity check and not a
/// defence against a compromised release — it catches the truncated or
/// proxy-mangled download *before* it is copied over a working install, which
/// is the failure that would otherwise leave an app that cannot start.
fn verify(archive: &Path, sha_url: &str) -> Result<(), String> {
    let response = client()?
        .get(sha_url)
        .send()
        .map_err(|_| "Could not reach github.com for the checksum.".to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub answered {} for the checksum.", response.status()));
    }
    let published = response.text().map_err(|e| e.to_string())?;
    // `<hex>  <filename>`, as `release-desktop.yml` writes it.
    let expected = published.split_whitespace().next().unwrap_or_default().to_lowercase();

    let bytes = std::fs::read(archive).map_err(|e| e.to_string())?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if expected != actual {
        return Err(format!(
            "The download does not match its published checksum ({actual} vs {expected}); \
             nothing was replaced."
        ));
    }
    Ok(())
}

/// Unpack the zip, shelling out rather than adding an inflate stack.
///
/// **Windows names `System32\tar.exe` by absolute path, never bare `tar`.**
/// Windows 10+ ships bsdtar there, which reads zip; a bare `tar` may resolve to
/// the GNU tar in a git-bash or MSYS on PATH, which answers "This does not look
/// like a tar archive". `managed_server.rs` in the daemon learned this the same
/// way and does the same thing.
fn unpack(archive: &Path, dir: &Path) -> Result<(), String> {
    let tar = if cfg!(windows) { r"C:\Windows\System32\tar.exe" } else { "tar" };
    let status = std::process::Command::new(tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()
        .map_err(|e| format!("Could not run {tar} to unpack the update: {e}"))?;
    if !status.success() {
        return Err(format!("Unpacking the update failed ({status})."));
    }
    Ok(())
}

/// Both exes inside the unpacked archive. The zip has its files at the root
/// (`dist` requires it), so this is a lookup rather than a walk — and a miss
/// means the artifact changed shape, which is worth saying plainly.
fn locate(staging: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    for name in [APP_EXE, DAEMON_EXE] {
        let path = staging.join(name);
        if !path.is_file() {
            return Err(format!("The downloaded archive has no {name} at its root."));
        }
        found.push(path);
    }
    Ok(found)
}

/// Move each staged binary into the install directory, sidelining what is
/// there. Windows will not let a running `.exe` be deleted or overwritten, but
/// it will let it be **renamed** — so the live file becomes `<name>.old` and
/// the new one takes the name it left.
///
/// A failure after the first rename puts back what it moved: a directory with
/// one new exe and one old one is a version skew across the wire contract, and
/// a half-updated install that starts is worse than one that never changed.
fn swap(staged: &[PathBuf], install_dir: &Path) -> Result<(), String> {
    let mut undo: Vec<(PathBuf, PathBuf)> = Vec::new();
    for source in staged {
        let name = source.file_name().unwrap_or_default();
        let dest = install_dir.join(name);
        let sidelined = install_dir.join(format!("{}.old", name.to_string_lossy()));
        let _ = std::fs::remove_file(&sidelined);

        if dest.exists() {
            if let Err(e) = std::fs::rename(&dest, &sidelined) {
                rollback(&undo);
                return Err(format!("Could not move the running {} aside: {e}", dest.display()));
            }
            undo.push((sidelined.clone(), dest.clone()));
        }
        if let Err(e) = std::fs::copy(source, &dest) {
            rollback(&undo);
            return Err(format!("Could not write the new {}: {e}", dest.display()));
        }
    }
    Ok(())
}

fn rollback(undo: &[(PathBuf, PathBuf)]) {
    for (sidelined, dest) in undo {
        let _ = std::fs::remove_file(dest);
        let _ = std::fs::rename(sidelined, dest);
    }
}

/// Delete the binaries an earlier update moved aside. Called at boot, which is
/// the first moment nothing holds them open — the process that was running from
/// the renamed file has exited by then.
pub fn sweep_old(install_dir: &Path) {
    for name in [APP_EXE, DAEMON_EXE] {
        let _ = std::fs::remove_file(install_dir.join(format!("{name}.old")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Release {
        Release { tag_name: tag.to_string(), draft: false, prerelease: false }
    }

    #[test]
    fn picks_the_highest_version_and_ignores_everything_else() {
        let releases = vec![
            release("v0.1.9"),
            release("v0.3.0"),
            // Out of publish order on purpose: the pick is by version.
            release("v0.2.5"),
            // Neither a draft nor a prerelease is something to upgrade to.
            Release { tag_name: "v9.0.0".into(), draft: true, prerelease: false },
            Release { tag_name: "v8.0.0".into(), draft: false, prerelease: true },
            // Not this series at all — the abandoned second one. It must not be
            // read as a 9.x of this product.
            release("desktop-v9.9.9"),
        ];
        assert_eq!(pick_newer("0.2.0", &releases).as_deref(), Some("0.3.0"));
        // Nothing published is above this build.
        assert_eq!(pick_newer("0.3.0", &releases), None);
        assert_eq!(pick_newer("1.0.0", &releases), None);
    }

    #[test]
    fn an_unparseable_tag_never_wins() {
        let releases = vec![release("vnightly"), release("v0.2.0")];
        assert_eq!(pick_newer("0.2.0", &releases), None);
    }

    #[test]
    fn compares_numerically_not_lexically() {
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
    }

    /// A scratch dir under the OS temp root, named for the test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agent-platform-update-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn swap_sidelines_the_old_binaries_and_installs_the_new() {
        let root = scratch("swap");
        let (staging, install) = (root.join("staging"), root.join("install"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&install).unwrap();
        for name in [APP_EXE, DAEMON_EXE] {
            std::fs::write(staging.join(name), b"new").unwrap();
            std::fs::write(install.join(name), b"old").unwrap();
        }

        swap(&locate(&staging).unwrap(), &install).unwrap();

        for name in [APP_EXE, DAEMON_EXE] {
            assert_eq!(std::fs::read(install.join(name)).unwrap(), b"new");
            // Renamed, not deleted — the running process is still executing it.
            let sidelined = install.join(format!("{name}.old"));
            assert_eq!(std::fs::read(&sidelined).unwrap(), b"old");
        }

        sweep_old(&install);
        assert!(!install.join(format!("{APP_EXE}.old")).exists());
        assert!(!install.join(format!("{DAEMON_EXE}.old")).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failure_partway_puts_back_what_it_moved() {
        let root = scratch("rollback");
        let (staging, install) = (root.join("staging"), root.join("install"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(staging.join(APP_EXE), b"new").unwrap();
        std::fs::write(install.join(APP_EXE), b"old").unwrap();
        std::fs::write(install.join(DAEMON_EXE), b"old").unwrap();

        // The daemon never made it into staging: the second copy fails, and the
        // first must not be left swapped — that is the version skew.
        let staged = vec![staging.join(APP_EXE), staging.join(DAEMON_EXE)];
        assert!(swap(&staged, &install).is_err());

        assert_eq!(std::fs::read(install.join(APP_EXE)).unwrap(), b"old");
        assert_eq!(std::fs::read(install.join(DAEMON_EXE)).unwrap(), b"old");
        assert!(!install.join(format!("{APP_EXE}.old")).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
