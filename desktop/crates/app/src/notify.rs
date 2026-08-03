//! Desktop toast notifications for job/run completion. Best-effort: a failure
//! to show a notification (no notification daemon, headless CI, etc.) is
//! logged and otherwise ignored — it must never take down the app.

pub fn job_finished(title: &str, label: &str, status: &str) {
    let body = format!("{label}: {status}");
    if let Err(e) = notify_rust::Notification::new()
        .summary(title)
        .body(&body)
        .appname("Agent Platform")
        .show()
    {
        eprintln!("[notify] failed to show notification: {e}");
    }
}
