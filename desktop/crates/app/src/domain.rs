//! Pure view-model helpers ported from `web/src/lib/*` — board rows, execution
//! waves, timeline rows, status→tone mapping, relative timestamps.
//!
//! Everything here is a pure function with tests; the iced views stay thin.

use crate::ui::Tone;
use agent_platform_client::types::{PlannerDag, SubagentNode, TaskNodeRecord};
use std::collections::{HashMap, HashSet};

/// Board columns (`web/src/lib/dagTasks.ts` BOARD_STATUSES).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoardColumn {
    Pending,
    Running,
    AwaitingReview,
    Completed,
    Failed,
}

impl BoardColumn {
    pub const ALL: [BoardColumn; 5] = [
        BoardColumn::Pending,
        BoardColumn::Running,
        BoardColumn::AwaitingReview,
        BoardColumn::Completed,
        BoardColumn::Failed,
    ];

    /// Short label matching the web board's column names.
    pub fn label(self) -> &'static str {
        match self {
            BoardColumn::Pending => "Pending",
            BoardColumn::Running => "Running",
            BoardColumn::AwaitingReview => "Review",
            BoardColumn::Completed => "Done",
            BoardColumn::Failed => "Failed",
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            BoardColumn::Pending => Tone::Neutral,
            BoardColumn::Running => Tone::Info,
            BoardColumn::AwaitingReview => Tone::Warning,
            BoardColumn::Completed => Tone::Success,
            BoardColumn::Failed => Tone::Danger,
        }
    }
}

/// Unknown statuses fall back to pending, as the web client does.
pub fn normalize_task_status(raw: &str) -> BoardColumn {
    match raw.to_lowercase().as_str() {
        "running" => BoardColumn::Running,
        "awaiting_review" => BoardColumn::AwaitingReview,
        "completed" => BoardColumn::Completed,
        "failed" => BoardColumn::Failed,
        _ => BoardColumn::Pending,
    }
}

/// Process status → badge tone (`web/src/lib/processStatusBadge.ts`).
pub fn process_status_tone(status: &str) -> Tone {
    match status {
        "failed" => Tone::Danger,
        "completed" => Tone::Success,
        "running" | "planning" => Tone::Info,
        "approval_required" | "task_review_required" => Tone::Warning,
        _ => Tone::Neutral,
    }
}

/// Wire status → label a person can read. Unknown values pass through so a
/// newer server string still shows rather than going blank.
pub fn process_status_label(status: &str) -> &str {
    match status {
        "pending" => "Pending",
        "planning" => "Planning",
        "approval_required" => "Needs plan approval",
        "approved" => "Approved",
        "task_review_required" => "Needs task review",
        "running" => "Running",
        "completed" => "Done",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        other => other,
    }
}

/// What the user must do for the run to move again. `None` while the engine
/// is still working or the run is finished.
pub fn process_waiting_hint(status: &str) -> Option<&'static str> {
    match status {
        "approval_required" => Some("Approve the plan to continue"),
        "task_review_required" => Some("Review a task to continue"),
        _ => None,
    }
}

/// A planner subagent joined to its task row (absent until the task exists).
#[derive(Debug, Clone)]
pub struct BoardRow {
    pub subagent: SubagentNode,
    pub task: Option<TaskNodeRecord>,
    pub column: BoardColumn,
}

fn synthetic_subagent(t: &TaskNodeRecord) -> SubagentNode {
    SubagentNode {
        client_uuid: t.client_uuid.clone(),
        role: t.role.clone(),
        system_prompt: t.system_prompt.clone(),
        instructions: t.instructions.clone(),
        dependencies: None,
        model: t.llm_model.clone(),
        subdecompose: None,
        requires_review: t.requires_review,
    }
}

/// One row per planner subagent; without a DAG, one row per task.
pub fn board_rows_from_dag(dag: Option<&PlannerDag>, tasks: &[TaskNodeRecord]) -> Vec<BoardRow> {
    let by_uuid: HashMap<&str, &TaskNodeRecord> =
        tasks.iter().map(|t| (t.client_uuid.as_str(), t)).collect();

    match dag {
        None => tasks
            .iter()
            .map(|t| BoardRow {
                subagent: synthetic_subagent(t),
                column: normalize_task_status(&t.status),
                task: Some(t.clone()),
            })
            .collect(),
        Some(dag) => dag
            .subagents
            .iter()
            .map(|sub| {
                let task = by_uuid.get(sub.client_uuid.as_str()).map(|t| (*t).clone());
                let column = task
                    .as_ref()
                    .map(|t| normalize_task_status(&t.status))
                    .unwrap_or(BoardColumn::Pending);
                BoardRow { subagent: sub.clone(), task, column }
            })
            .collect(),
    }
}

/// Case-insensitive match over role, uuid, instructions and system prompt.
pub fn matches_board_search(row: &BoardRow, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let hay = format!(
        "{}\n{}\n{}\n{}",
        row.subagent.role, row.subagent.client_uuid, row.subagent.instructions,
        row.subagent.system_prompt
    )
    .to_lowercase();
    hay.contains(&q)
}

/// Rows that should appear in "needs attention" mode.
pub fn row_needs_attention(row: &BoardRow, process_status: &str) -> bool {
    row.column == BoardColumn::AwaitingReview
        || (row.column == BoardColumn::Pending && process_status == "approval_required")
}

/// Execution waves: each wave holds subagents whose internal deps are satisfied
/// by earlier waves. Bails out on a cycle rather than looping forever.
pub fn compute_waves(dag: &PlannerDag) -> Vec<Vec<String>> {
    let dep_map: HashMap<&str, HashSet<&str>> = dag
        .subagents
        .iter()
        .map(|s| {
            let deps = s
                .dependencies
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|d| d.as_str())
                .collect();
            (s.client_uuid.as_str(), deps)
        })
        .collect();

    // Insertion order preserved so waves are deterministic.
    let mut pending: Vec<&str> = dag.subagents.iter().map(|s| s.client_uuid.as_str()).collect();
    let mut waves = Vec::new();
    while !pending.is_empty() {
        let wave: Vec<&str> = pending
            .iter()
            .copied()
            .filter(|id| {
                dep_map[id]
                    .iter()
                    .filter(|d| dep_map.contains_key(*d))
                    .all(|d| !pending.contains(d))
            })
            .collect();
        if wave.is_empty() {
            break; // cycle
        }
        pending.retain(|id| !wave.contains(id));
        waves.push(wave.into_iter().map(String::from).collect());
    }
    waves
}

#[derive(Debug, Clone)]
pub struct TimelineRow {
    pub wave_index: usize,
    pub client_uuid: String,
    pub role: String,
    pub column: BoardColumn,
}

pub fn build_timeline_rows(
    dag: Option<&PlannerDag>,
    tasks: &[TaskNodeRecord],
) -> Vec<TimelineRow> {
    let by_uuid: HashMap<&str, &TaskNodeRecord> =
        tasks.iter().map(|t| (t.client_uuid.as_str(), t)).collect();

    let Some(dag) = dag else {
        return tasks
            .iter()
            .map(|t| TimelineRow {
                wave_index: 0,
                client_uuid: t.client_uuid.clone(),
                role: t.role.clone(),
                column: normalize_task_status(&t.status),
            })
            .collect();
    };

    let mut rows = Vec::new();
    for (wave_index, ids) in compute_waves(dag).into_iter().enumerate() {
        for id in ids {
            let role = dag
                .subagents
                .iter()
                .find(|s| s.client_uuid == id)
                .map(|s| s.role.clone())
                .unwrap_or_else(|| id.clone());
            let column = by_uuid
                .get(id.as_str())
                .map(|t| normalize_task_status(&t.status))
                .unwrap_or(BoardColumn::Pending);
            rows.push(TimelineRow { wave_index, client_uuid: id, role, column });
        }
    }
    rows
}

pub fn parse_task_dependencies(task: &TaskNodeRecord) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&task.dependencies_json).unwrap_or_default()
}

/// Clip to `max` *characters* — not bytes, so a multi-byte name cannot be cut
/// mid-codepoint — with an ellipsis standing in for what was dropped. The
/// ellipsis is outside the budget, as three screens independently decided.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Shortened uuid for dense UI (`web/src/api/dag.ts` shortUuid).
pub fn short_uuid(uuid: &str) -> String {
    truncate(uuid.trim(), 8)
}

/// A trimmed field, or `None` when the user left it blank. Every screen with an
/// optional text input wants this before it builds a request body: the server
/// reads `""` as "set it to empty", which is not what an untouched field means.
pub fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Flatten a client error into the `String` every screen's `error` field holds.
/// `Task::perform` needs an owned, `'static` result, and no screen has ever
/// matched on the error's variant.
pub fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

/// A size on disk: TB, GB or MB, whichever leaves a number you can read.
///
/// Shared by Model ops, the Ollama provider dialog and the machine meters. The
/// TB arm is the meters' — model weights never reach it, but a volume does, and
/// "1514.6 GB free" is a figure you have to divide before it means anything.
pub fn format_size(bytes: i64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TB: f64 = GB * 1024.0;
    let b = bytes as f64;
    if b >= TB {
        format!("{:.1} TB", b / TB)
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / (1024.0 * 1024.0))
    }
}

/// Coarse relative time from an ISO timestamp. The server emits naive local
/// timestamps (no zone), so they are compared against local now.
pub fn relative_time(iso: &str) -> Option<String> {
    let secs = seconds_since(iso)?;
    let abs = secs.abs();
    Some(match abs {
        s if s < 10 => "just now".to_string(),
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 172_800 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    })
}

/// "Finished …" for terminal rows (completed_at), "Started …" otherwise.
pub fn relative_task_activity(row: &BoardRow) -> Option<String> {
    let task = row.task.as_ref()?;
    let terminal = matches!(row.column, BoardColumn::Completed | BoardColumn::Failed);
    let iso = if terminal { task.completed_at.as_ref() } else { task.started_at.as_ref() }?;
    let rel = relative_time(iso)?;
    Some(if terminal { format!("Finished {rel}") } else { format!("Started {rel}") })
}

/// How long a task ran, from its own two timestamps. `None` until it finishes —
/// a running node's elapsed changes every frame and belongs in relative time.
pub fn task_duration_secs(task: &TaskNodeRecord) -> Option<i64> {
    let (start, end) = (task.started_at.as_deref()?, task.completed_at.as_deref()?);
    let secs = iso_to_epoch_secs(end)? - iso_to_epoch_secs(start)?;
    (secs >= 0).then_some(secs)
}

/// `45s`, `3m 05s`, `2h 14m`.
pub fn compact_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Wall-clock a run has been going, or took. Falls back to nothing when the
/// timestamps do not parse rather than showing a wrong number.
pub fn process_elapsed(process: &agent_platform_client::types::ProcessRecord) -> Option<String> {
    let start = iso_to_epoch_secs(&process.created_at)?;
    let terminal = matches!(process.status.as_str(), "completed" | "failed" | "cancelled");
    let end = if terminal {
        iso_to_epoch_secs(&process.updated_at)?
    } else {
        chrono::Utc::now().timestamp()
    };
    (end >= start).then(|| compact_duration(end - start))
}

/// Seconds between an ISO-8601 timestamp and now. Parsed by hand: the only
/// shapes the server emits are `YYYY-MM-DDTHH:MM:SS[.ffffff][Z]`, so a date
/// crate would be a dependency for one format.
fn seconds_since(iso: &str) -> Option<i64> {
    let epoch = iso_to_epoch_secs(iso)?;
    // Server timestamps are naive **UTC** - `wire::sql_now` is
    // `Utc::now().naive_utc()`, and the `Z`-suffixed ones this also parses are
    // UTC by definition. Comparing against *local* now added the machine's
    // offset to every age on screen: at UTC+1 a run started two seconds ago
    // read "1h ago", which is also why the buckets looked too coarse.
    Some(chrono::Utc::now().timestamp() - epoch)
}

fn iso_to_epoch_secs(iso: &str) -> Option<i64> {
    let s = iso.trim().trim_end_matches('Z');
    let (date, time) = s.split_once('T').or_else(|| s.split_once(' '))?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) =
        (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let time = time.split('.').next()?;
    let mut t = time.split(':');
    let (hh, mm, ss): (i64, i64, i64) =
        (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next().unwrap_or("0").parse().ok()?);
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Howard Hinnant's days_from_civil: civil date → days since 1970-01-01.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dag(subs: &[(&str, &[&str])]) -> PlannerDag {
        serde_json::from_value(json!({
            "team_name": "t",
            "goal_restatement": "g",
            "subagents": subs.iter().map(|(id, deps)| json!({
                "client_uuid": id, "role": format!("role-{id}"),
                "system_prompt": "s", "instructions": "i", "dependencies": deps,
            })).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn task(uuid: &str, status: &str) -> TaskNodeRecord {
        serde_json::from_value(json!({
            "id": 1, "process_id": 1, "client_uuid": uuid, "role": "r",
            "system_prompt": "s", "instructions": "i", "llm_model": null,
            "dependencies_json": "[]", "status": status, "output": null,
            "tokens_used": 0, "started_at": null, "completed_at": null,
        }))
        .unwrap()
    }

    #[test]
    fn board_rows_join_tasks_and_default_to_pending() {
        let d = dag(&[("a", &[]), ("b", &["a"])]);
        let rows = board_rows_from_dag(Some(&d), &[task("a", "completed")]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].column, BoardColumn::Completed);
        assert_eq!(rows[1].column, BoardColumn::Pending);
        assert!(rows[1].task.is_none());
    }

    #[test]
    fn board_rows_without_dag_use_tasks() {
        let rows = board_rows_from_dag(None, &[task("a", "running")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].column, BoardColumn::Running);
    }

    #[test]
    fn unknown_status_is_pending() {
        assert_eq!(normalize_task_status("weird"), BoardColumn::Pending);
        assert_eq!(normalize_task_status("AWAITING_REVIEW"), BoardColumn::AwaitingReview);
    }

    #[test]
    fn process_status_label_humanizes_the_wire() {
        assert_eq!(process_status_label("task_review_required"), "Needs task review");
        assert_eq!(process_status_label("approval_required"), "Needs plan approval");
        assert_eq!(process_status_label("completed"), "Done");
        assert_eq!(process_status_label("mystery_status"), "mystery_status");
        assert_eq!(
            process_waiting_hint("approval_required"),
            Some("Approve the plan to continue")
        );
        assert_eq!(
            process_waiting_hint("task_review_required"),
            Some("Review a task to continue")
        );
        assert_eq!(process_waiting_hint("running"), None);
    }

    #[test]
    fn waves_group_by_dependency_depth() {
        let d = dag(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("e", &["b", "c"])]);
        let waves = compute_waves(&d);
        assert_eq!(waves, vec![vec!["a"], vec!["b", "c"], vec!["e"]]);
    }

    #[test]
    fn waves_bail_on_cycle_instead_of_hanging() {
        let d = dag(&[("a", &["b"]), ("b", &["a"])]);
        assert!(compute_waves(&d).is_empty());
    }

    #[test]
    fn external_dependencies_do_not_block() {
        // A dep that is not itself a subagent (sub-DAG parent) must be ignored.
        let d = dag(&[("a", &["outside"])]);
        assert_eq!(compute_waves(&d), vec![vec!["a"]]);
    }

    #[test]
    fn timeline_rows_carry_wave_index() {
        let d = dag(&[("a", &[]), ("b", &["a"])]);
        let rows = build_timeline_rows(Some(&d), &[task("b", "running")]);
        assert_eq!(rows[0].wave_index, 0);
        assert_eq!(rows[1].wave_index, 1);
        assert_eq!(rows[1].column, BoardColumn::Running);
    }

    #[test]
    fn search_matches_role_and_uuid() {
        let rows = board_rows_from_dag(Some(&dag(&[("abc", &[])])), &[]);
        assert!(matches_board_search(&rows[0], "role-abc"));
        assert!(matches_board_search(&rows[0], "ABC"));
        assert!(matches_board_search(&rows[0], "  "));
        assert!(!matches_board_search(&rows[0], "zzz"));
    }

    #[test]
    fn needs_attention_covers_review_and_pending_approval() {
        let rows = board_rows_from_dag(Some(&dag(&[("a", &[])])), &[task("a", "awaiting_review")]);
        assert!(row_needs_attention(&rows[0], "running"));
        let rows = board_rows_from_dag(Some(&dag(&[("a", &[])])), &[]);
        assert!(row_needs_attention(&rows[0], "approval_required"));
        assert!(!row_needs_attention(&rows[0], "running"));
    }

    #[test]
    fn iso_parsing_matches_known_epochs() {
        assert_eq!(iso_to_epoch_secs("1970-01-01T00:00:00"), Some(0));
        assert_eq!(iso_to_epoch_secs("2000-01-01T00:00:00Z"), Some(946_684_800));
        // Microseconds and space separator both appear in server payloads.
        assert_eq!(iso_to_epoch_secs("2026-08-02T21:21:50.327892"), iso_to_epoch_secs("2026-08-02 21:21:50"));
        assert_eq!(iso_to_epoch_secs("nonsense"), None);
    }

    #[test]
    fn short_uuid_truncates() {
        assert_eq!(short_uuid("abc"), "abc");
        assert_eq!(short_uuid("0123456789abcdef"), "01234567…");
    }

    #[test]
    fn dependencies_parse_tolerates_garbage() {
        let mut t = task("a", "pending");
        t.dependencies_json = "[\"x\",\"y\"]".into();
        assert_eq!(parse_task_dependencies(&t), vec!["x", "y"]);
        t.dependencies_json = "not json".into();
        assert!(parse_task_dependencies(&t).is_empty());
    }
}

/// A screen's transient message. Carries a generation counter so two
/// identical messages in a row are two toasts: the toast timer keys its
/// countdown on `(text, seq)`, and equal text alone would let the second one
/// inherit what was left of the first one's five seconds.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Toast {
    text: Option<String>,
    seq: u64,
    tone: Option<crate::ui::Tone>,
}

impl Toast {
    pub fn set(&mut self, text: impl Into<String>) {
        self.set_toned(text, crate::ui::Tone::Success);
    }

    /// An answer that is neither a failure nor something having happened — a
    /// call the server accepted and declined to act on. Green there reads as
    /// "done", which is the opposite of what it says.
    pub fn set_info(&mut self, text: impl Into<String>) {
        self.set_toned(text, crate::ui::Tone::Info);
    }

    fn set_toned(&mut self, text: impl Into<String>, tone: crate::ui::Tone) {
        self.text = Some(text.into());
        self.tone = Some(tone);
        self.seq = self.seq.wrapping_add(1);
    }

    pub fn clear(&mut self) {
        self.text = None;
    }

    /// The message, which showing of it this is, and how it should read;
    /// `None` when nothing is up.
    pub fn get(&self) -> Option<(String, u64, crate::ui::Tone)> {
        let tone = self.tone.unwrap_or(crate::ui::Tone::Success);
        self.text.clone().map(|t| (t, self.seq, tone))
    }

    pub fn is_none(&self) -> bool {
        self.text.is_none()
    }
}

#[cfg(test)]
mod toast_tests {
    use super::Toast;

    /// The counter is the whole point: the same sentence twice must not look
    /// like one unchanged toast to the timer keyed on it.
    #[test]
    fn the_same_message_twice_is_two_toasts() {
        let mut t = Toast::default();
        t.set("Project deleted.");
        let first = t.get().expect("a message is up");
        t.set("Project deleted.");
        let second = t.get().unwrap();
        assert_eq!(first.0, second.0);
        assert_ne!(first.1, second.1, "the second showing gets its own countdown");

        t.clear();
        assert!(t.is_none() && t.get().is_none());
    }
}
