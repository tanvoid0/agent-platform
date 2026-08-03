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

/// Shortened uuid for dense UI (`web/src/api/dag.ts` shortUuid).
pub fn short_uuid(uuid: &str) -> String {
    let t = uuid.trim();
    if t.chars().count() <= 8 {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(8).collect::<String>())
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

/// Seconds between an ISO-8601 timestamp and now. Parsed by hand: the only
/// shapes the server emits are `YYYY-MM-DDTHH:MM:SS[.ffffff][Z]`, so a date
/// crate would be a dependency for one format.
fn seconds_since(iso: &str) -> Option<i64> {
    let epoch = iso_to_epoch_secs(iso)?;
    // Server timestamps are naive local time, so compare against local now.
    Some(chrono::Local::now().naive_local().and_utc().timestamp() - epoch)
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
