//! Desktop toast notifications for work that finished while the user was
//! looking somewhere else. Best-effort: a failure to show one (no notification
//! daemon, headless CI, etc.) is logged and otherwise ignored — it must never
//! take down the app.

use std::sync::Mutex;

/// The surface the user is actually looking at, as the same key a finishing job
/// passes to [`away`]; empty when the window is hidden or unfocused.
///
/// Global because the jobs that finish do so deep inside their own module's
/// `update`, which has no `App` to ask. `main::update` rewrites it after every
/// message, so it cannot go stale.
static WATCHING: Mutex<&'static str> = Mutex::new("");

fn watched() -> std::sync::MutexGuard<'static, &'static str> {
    // A panic elsewhere must not cost the user every future notification.
    WATCHING.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn watching(key: &'static str) {
    *watched() = key;
}

/// Toast `body` unless the user is already watching `key` — work that finishes
/// in plain sight has announced itself.
pub fn away(key: &'static str, title: &str, body: &str) {
    if *watched() == key {
        return;
    }
    if let Err(e) = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .appname("Agent Platform")
        .show()
    {
        eprintln!("[notify] failed to show notification: {e}");
    }
}
