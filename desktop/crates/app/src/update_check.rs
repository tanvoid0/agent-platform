//! "Is there a newer build?" — the check half of an updater.
//!
//! Deliberately not the install half. Replacing a running `.exe` on Windows is
//! its own problem (the file is locked; it needs a rename-then-swap), and there
//! has never been a published release to test a download against. What this
//! does is cheap, safe and enough to stop someone running a build from months
//! ago without knowing: ask GitHub for the newest `v*` tag, compare it to this
//! binary's own version, and offer to open the release page.
//!
//! The daemon has a real self-updater already — `dist` generates an
//! `agent-platform-server-<target>-update` binary per platform (see
//! `dist-workspace.toml`). This is the app's half, and it is smaller on
//! purpose.

use serde::Deserialize;

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
}
