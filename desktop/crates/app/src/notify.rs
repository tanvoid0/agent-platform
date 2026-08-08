//! Notifications for work that finished — or stopped and is waiting — while the
//! user was looking somewhere else.
//!
//! Two surfaces, one call: a desktop toast (best-effort; a failure to show one
//! — no notification daemon, headless CI — is logged and otherwise ignored) and
//! an in-app inbox the sidebar counts and the bell panel lists. The toast is
//! gone in seconds; the inbox is what is still there when the user comes back
//! from lunch.
//!
//! Both are suppressed for the surface the user is actually watching: work that
//! finishes in plain sight has announced itself, and a badge on the screen you
//! are already on is noise.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// The surface the user is actually looking at, as the same key a finishing job
/// passes to [`away`]; empty when the window is hidden or unfocused.
///
/// Global because the jobs that finish do so deep inside their own module's
/// `update`, which has no `App` to ask. `main::update` rewrites it after every
/// message, so it cannot go stale.
static WATCHING: Mutex<&'static str> = Mutex::new("");

/// What a note is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The work ended — a reply landed, a run reached a terminal status.
    Done,
    /// The work stopped and cannot continue without the user: an approval, a
    /// task review. The one that is worth interrupting for.
    Review,
}

/// One thing that happened off-screen, kept until the user goes back to the
/// screen it belongs to (or dismisses it).
#[derive(Debug, Clone)]
pub struct Note {
    pub id: u64,
    /// The screen key, same vocabulary as [`watching`] — `main` maps it back to
    /// a `Screen` so clicking the note navigates there.
    pub key: &'static str,
    pub kind: Kind,
    pub title: String,
    pub body: String,
}

/// Oldest notes fall off past this. A user who has been away for a day does not
/// want a thousand-row panel, and the badge count is the signal anyway.
const CAPACITY: usize = 100;

static INBOX: Mutex<Vec<Note>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn watched() -> std::sync::MutexGuard<'static, &'static str> {
    // A panic elsewhere must not cost the user every future notification.
    WATCHING.lock().unwrap_or_else(|e| e.into_inner())
}

fn inbox() -> std::sync::MutexGuard<'static, Vec<Note>> {
    INBOX.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn watching(key: &'static str) {
    *watched() = key;
}

/// Announce finished work unless the user is already watching `key`.
pub fn away(key: &'static str, title: &str, body: &str) {
    post(key, Kind::Done, title, body);
}

/// The same, for work that is *waiting* on the user rather than done with them.
pub fn review(key: &'static str, title: &str, body: &str) {
    post(key, Kind::Review, title, body);
}

fn post(key: &'static str, kind: Kind, title: &str, body: &str) {
    if *watched() == key {
        return;
    }
    let mut notes = inbox();
    notes.push(Note {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        key,
        kind,
        title: title.to_string(),
        body: body.to_string(),
    });
    let overflow = notes.len().saturating_sub(CAPACITY);
    notes.drain(..overflow);
    drop(notes);
    toast(title, body);
}

#[cfg(not(test))]
fn toast(title: &str, body: &str) {
    if let Err(e) = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .appname("Agent Platform")
        .show()
    {
        eprintln!("[notify] failed to show notification: {e}");
    }
}

/// The inbox is what the tests are about; a hundred real desktop toasts during
/// `cargo test` are not.
#[cfg(test)]
fn toast(_title: &str, _body: &str) {}

/// Everything unseen, newest first — the bell panel's list.
pub fn notes() -> Vec<Note> {
    let mut notes = inbox().clone();
    notes.reverse();
    notes
}

/// Unseen notes for one screen, for its sidebar badge.
pub fn count(key: &str) -> usize {
    inbox().iter().filter(|n| n.key == key).count()
}

/// Unseen notes across every screen, for the bell.
pub fn total() -> usize {
    inbox().len()
}

/// Whether anything unseen is *waiting* on the user rather than merely
/// finished — the badge is louder when it is. An empty key asks about every
/// screen at once, for the bell.
pub fn review_waiting(key: &str) -> bool {
    inbox().iter().any(|n| n.kind == Kind::Review && (key.is_empty() || n.key == key))
}

/// Mark a screen's notes seen. Called with whatever the user is now looking at,
/// so arriving on a screen clears its badge; the empty key (window hidden or
/// behind another app) matches nothing and clears nothing.
pub fn seen(key: &str) {
    inbox().retain(|n| n.key != key);
}

/// The screen one note points at, and forget the note — clicking it is seeing it.
pub fn take(id: u64) -> Option<&'static str> {
    let mut notes = inbox();
    let at = notes.iter().position(|n| n.id == id)?;
    Some(notes.remove(at).key)
}

pub fn clear() {
    inbox().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inbox is global, so this is one test rather than several — parallel
    /// tests would see each other's notes.
    #[test]
    fn notes_are_counted_per_screen_and_cleared_by_visiting_it() {
        clear();
        watching("");

        away("processes", "Run #1", "completed");
        review("coder", "Coder", "waiting for approval");
        away("processes", "Run #2", "failed");
        assert_eq!(total(), 3);
        assert_eq!(count("processes"), 2);
        assert!(review_waiting(""));
        assert!(review_waiting("coder"));
        assert!(!review_waiting("processes"));

        // Newest first, so the panel reads like a feed.
        assert_eq!(notes().first().map(|n| n.title.clone()), Some("Run #2".to_string()));

        // Work that finishes on the screen you are watching is not news.
        watching("processes");
        away("processes", "Run #3", "completed");
        assert_eq!(count("processes"), 2);

        // Arriving on a screen clears only that screen.
        seen("processes");
        assert_eq!(count("processes"), 0);
        assert_eq!(total(), 1);

        // Clicking a note says where to go and forgets it.
        let id = notes()[0].id;
        assert_eq!(take(id), Some("coder"));
        assert_eq!(total(), 0);
        assert!(take(id).is_none());

        // The empty key (hidden window) clears nothing.
        watching("");
        away("workflows", "Run #4", "succeeded");
        seen("");
        assert_eq!(total(), 1);
        clear();
        assert_eq!(total(), 0);

        // Bounded: the oldest fall off, not the newest.
        for i in 0..(CAPACITY + 10) {
            away("workflows", &format!("Run #{i}"), "succeeded");
        }
        assert_eq!(total(), CAPACITY);
        assert_eq!(notes()[0].title, format!("Run #{}", CAPACITY + 9));
        clear();
    }
}
