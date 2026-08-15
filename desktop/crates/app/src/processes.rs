//! Processes screen state and update logic (Phase 3).
//!
//! Polling cadences mirror the web client: list 3s, detail 800ms while a run is
//! live / 4s once settled. SSE frames are treated as "refetch now" triggers, and
//! the subscription is gated on the polled status exactly as the web hook was —
//! a terminal process replaying a backlog closes without a sentinel.

use crate::domain::{err_string, non_empty};
use crate::domain::{self, BoardColumn, BoardRow};
use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::Task;
use std::time::Duration;

/// Detail sub-views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Graph,
    Board,
    Timeline,
    Events,
}

impl ViewMode {
    pub const ALL: [ViewMode; 4] =
        [ViewMode::Graph, ViewMode::Board, ViewMode::Timeline, ViewMode::Events];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Graph => "Graph",
            ViewMode::Board => "Board",
            ViewMode::Timeline => "Timeline",
            ViewMode::Events => "Events",
        }
    }
}

/// Which task the review modal is acting on.
#[derive(Debug, Clone)]
pub struct ReviewDraft {
    pub task_id: i64,
    pub role: String,
    pub output: String,
    pub feedback: String,
    pub instructions: String,
}

#[derive(Default)]
pub struct Composer {
    pub goal: String,
    pub team_id: Option<i64>,
    pub project_id: Option<i64>,
    pub auto_approve: bool,
    pub submitting: bool,
}

pub struct State {
    pub processes: Vec<ProcessRecord>,
    pub selected: Option<i64>,
    pub detail: Option<ProcessDetailResponse>,
    pub events: Vec<EventLogRecord>,
    pub teams: Vec<TeamTemplateSummary>,
    pub projects: Vec<ProjectSummary>,
    /// False until each list has come back once; an empty list is a valid answer,
    /// so emptiness alone cannot drive the retry.
    lists_loaded: (bool, bool),
    pub composer: Composer,
    pub view: ViewMode,
    pub board_search: String,
    pub needs_attention_only: bool,
    pub event_filter: String,
    pub inspecting: Option<String>,
    pub review: Option<ReviewDraft>,
    pub lineage: crate::graph::Lineage,
    pub viewport: crate::graph::Viewport,
    pub error: Option<String>,
    pub notice: crate::domain::Toast,
    pub busy: bool,
    /// One chat thread per scope (`"<run id>"`, or `"<run id>:<uuid>"` while a
    /// subagent is inspected), so switching runs does not mix conversations.
    /// In memory only — the server's chat endpoint is stateless either way.
    pub chats: std::collections::HashMap<String, crate::chat::State>,
    pub chat_open: bool,
    /// The app-wide provider/model default, refreshed by `main` before every
    /// message. Each thread copies it on its first turn and keeps that pair.
    pub chat_default: (String, String),
}

impl Default for State {
    fn default() -> Self {
        Self {
            processes: Vec::new(),
            selected: None,
            detail: None,
            events: Vec::new(),
            teams: Vec::new(),
            projects: Vec::new(),
            lists_loaded: (false, false),
            composer: Composer::default(),
            view: ViewMode::Graph,
            board_search: String::new(),
            needs_attention_only: false,
            event_filter: String::new(),
            inspecting: None,
            review: None,
            lineage: crate::graph::Lineage::All,
            viewport: crate::graph::Viewport::default(),
            error: None,
            notice: Default::default(),
            busy: false,
            chats: std::collections::HashMap::new(),
            chat_open: false,
            chat_default: (String::new(), String::new()),
        }
    }
}

/// How much of a task's output travels with the scope context. Same bound the
/// web panel used — enough to reason about, short of blowing the prompt.
const OUTPUT_SNIP_LEN: usize = 3000;

impl State {
    pub fn selected_process(&self) -> Option<&ProcessRecord> {
        self.detail.as_ref().map(|d| &d.process)
    }

    pub fn status_str(&self) -> &str {
        self.selected_process().map(|p| p.status.as_str()).unwrap_or("")
    }

    pub fn dag(&self) -> Option<PlannerDag> {
        let p = self.selected_process()?;
        agent_platform_client::dag::parse_planner_dag(p.dag_json.as_deref())
    }

    pub fn board_rows(&self) -> Vec<BoardRow> {
        let Some(detail) = &self.detail else { return Vec::new() };
        let dag = self.dag();
        let mut rows = domain::board_rows_from_dag(dag.as_ref(), &detail.tasks);
        if !self.board_search.trim().is_empty() {
            rows.retain(|r| domain::matches_board_search(r, &self.board_search));
        }
        if self.needs_attention_only {
            let status = self.status_str().to_string();
            rows.retain(|r| domain::row_needs_attention(r, &status));
        }
        rows
    }

    pub fn rows_in_column(&self, column: BoardColumn) -> Vec<BoardRow> {
        self.board_rows().into_iter().filter(|r| r.column == column).collect()
    }

    /// The run is live: poll fast and keep a stream open.
    pub fn is_live(&self) -> bool {
        matches!(
            self.selected_process().map(|p| p.status),
            Some(ProcessStatus::Pending)
                | Some(ProcessStatus::Planning)
                | Some(ProcessStatus::Approved)
                | Some(ProcessStatus::Running)
        )
    }

    /// SSE is only opened for live runs — the server closes the stream on
    /// terminal and approval states, and a terminal run with a log backlog
    /// closes without a sentinel (would reconnect forever otherwise).
    pub fn stream_eligible(&self) -> bool {
        self.is_live()
    }

    pub fn detail_poll_interval(&self) -> Duration {
        if self.is_live() {
            Duration::from_millis(800)
        } else {
            Duration::from_secs(4)
        }
    }

    /// Graph nodes for the selected run under the current lineage cap.
    pub fn graph_layout(&self) -> crate::graph::GraphLayout {
        let Some(detail) = &self.detail else { return Default::default() };
        let Some(dag) = self.dag() else { return Default::default() };
        crate::graph::dag_layout(&dag.subagents, &detail.tasks, self.lineage)
    }

    pub fn task_by_uuid(&self, uuid: &str) -> Option<&TaskNodeRecord> {
        self.detail.as_ref()?.tasks.iter().find(|t| t.client_uuid == uuid)
    }

    /// Which thread the chat panel is talking on: the inspected subagent if one
    /// is open, otherwise the run. `None` with nothing selected.
    pub fn chat_key(&self) -> Option<String> {
        let id = self.selected_process()?.id;
        Some(match &self.inspecting {
            Some(uuid) => format!("{id}:{uuid}"),
            None => id.to_string(),
        })
    }

    /// Make sure the current scope has a thread, so the panel has something to
    /// render before the first message is typed.
    pub fn ensure_chat(&mut self) {
        if let Some(key) = self.chat_key() {
            self.chats.entry(key.clone()).or_insert_with(|| crate::chat::State::scoped(&key));
        }
    }

    /// The scope context sent ahead of the thread. Rebuilt per send so a run
    /// that has moved on since the last question is described as it is now.
    pub fn scope_system(&self) -> Option<String> {
        let process = self.selected_process()?;
        let mut parts = match &self.inspecting {
            None => vec!["You are a concise assistant for the agent-platform orchestration UI."
                .to_string()],
            Some(_) => vec![
                "You are a concise assistant for a single subagent task in agent-platform."
                    .to_string(),
            ],
        };
        parts.push(format!("Process id: {}", process.id));
        parts.push(format!("Status: {}", process.status.as_str()));
        parts.push(format!("Goal: {}", process.goal));
        if let Some(reason) = process.failure_reason.as_deref().map(str::trim).filter(|r| !r.is_empty())
        {
            parts.push(format!("Last failure: {reason}"));
        }

        match &self.inspecting {
            None => parts.push(
                "Help interpret DAG tasks, statuses, and next steps. Do not invent task outputs."
                    .to_string(),
            ),
            Some(uuid) => {
                parts.push(format!("Focused client_uuid: {uuid}"));
                if let Some(sub) = self
                    .dag()
                    .and_then(|d| d.subagents.into_iter().find(|s| &s.client_uuid == uuid))
                {
                    parts.push(format!("Role: {}", sub.role));
                }
                if let Some(task) = self.task_by_uuid(uuid) {
                    parts.push(format!("Task status: {}", task.status.as_str()));
                    if let Some(output) =
                        task.output.as_deref().map(str::trim).filter(|o| !o.is_empty())
                    {
                        let snip = match output.char_indices().nth(OUTPUT_SNIP_LEN) {
                            Some((cut, _)) => format!("{}…", &output[..cut]),
                            None => output.to_string(),
                        };
                        parts.push(format!("Task output (snippet): {snip}"));
                    }
                }
                parts.push(
                    "Answer about this task only. Do not invent output it does not have."
                        .to_string(),
                );
            }
        }
        Some(parts.join("\n"))
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    // data in
    ListTick,
    Listed(Result<Vec<ProcessRecord>, String>),
    Select(i64),
    DetailTick,
    Detailed(Result<Box<ProcessDetailResponse>, String>),
    EventsLoaded(Result<Vec<EventLogRecord>, String>),
    TeamsLoaded(Result<Vec<TeamTemplateSummary>, String>),
    ProjectsLoaded(Result<Vec<ProjectSummary>, String>),
    StreamFrame,
    // composer
    GoalChanged(String),
    TeamPicked(i64),
    ProjectPicked(Option<i64>),
    ToggleAutoApprove(bool),
    Submit,
    Created(Result<i64, String>),
    // detail controls
    SetView(ViewMode),
    BoardSearchChanged(String),
    ToggleNeedsAttention(bool),
    EventFilterChanged(String),
    Inspect(Option<String>),
    SetLineage(crate::graph::Lineage),
    Canvas(crate::graph::CanvasEvent),
    // actions
    Approve,
    Cancel,
    Retry,
    Sync,
    Export,
    RetryTask(i64),
    OpenReview(i64),
    CloseReview,
    ReviewOutputChanged(String),
    ReviewFeedbackChanged(String),
    ReviewInstructionsChanged(String),
    SubmitReview(ReviewDecision),
    ActionDone(Result<String, String>),
    DismissNotice,
    // scoped chat
    ToggleChat,
    Chat(crate::chat::Message),
}

impl From<crate::graph::CanvasEvent> for Message {
    fn from(event: crate::graph::CanvasEvent) -> Self {
        Message::Canvas(event)
    }
}

pub fn load_lists(client: &Client) -> Task<Message> {
    let c1 = client.clone();
    let c2 = client.clone();
    Task::batch([
        Task::perform(async move { err_string(c1.teams().await).map(|r| r.teams) }, Message::TeamsLoaded),
        Task::perform(
            async move { err_string(c2.projects().await).map(|r| r.projects) },
            Message::ProjectsLoaded,
        ),
    ])
}

fn fetch_list(client: &Client, project_id: Option<i64>) -> Task<Message> {
    let client = client.clone();
    // `GET /processes` requires an explicit scope; the composer's project
    // selection doubles as the list scope, unassigned by default.
    let filter = match project_id {
        Some(id) => ProcessListFilter::Project(id),
        None => ProcessListFilter::Unassigned,
    };
    Task::perform(
        async move { err_string(client.processes(30, filter).await).map(|r| r.processes) },
        Message::Listed,
    )
}

fn fetch_detail(client: &Client, id: i64) -> Task<Message> {
    let c1 = client.clone();
    let c2 = client.clone();
    Task::batch([
        Task::perform(
            async move { err_string(c1.process_detail(id).await).map(Box::new) },
            Message::Detailed,
        ),
        Task::perform(
            async move { err_string(c2.process_events(id, None, 2000, 0).await).map(|r| r.events) },
            Message::EventsLoaded,
        ),
    ])
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::ListTick => {
            // Teams/projects are fetched at boot, before the app's own server has
            // finished starting — that first request fails. Retry on the list
            // poll until they arrive, or the composer has no options to pick.
            let mut tasks = vec![fetch_list(client, state.composer.project_id)];
            if !(state.lists_loaded.0 && state.lists_loaded.1) {
                tasks.push(load_lists(client));
            }
            Task::batch(tasks)
        }
        Message::Listed(Ok(list)) => {
            state.error = None;
            state.processes = list;
            // Auto-select the newest run so the pane is never blank on first load.
            if state.selected.is_none() {
                if let Some(first) = state.processes.first().map(|p| p.id) {
                    return update(state, client, Message::Select(first));
                }
            }
            Task::none()
        }
        Message::Listed(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::Select(id) => {
            state.selected = Some(id);
            state.detail = None;
            state.events.clear();
            state.inspecting = None;
            state.review = None;
            state.viewport = crate::graph::Viewport::default();
            fetch_detail(client, id)
        }
        Message::DetailTick | Message::StreamFrame => match state.selected {
            Some(id) => fetch_detail(client, id),
            None => Task::none(),
        },
        Message::Detailed(Ok(detail)) => {
            let previous = state.selected_process().map(|p| p.status);
            if let Some(kind) = settled(previous, detail.process.status) {
                let title = format!("Run #{}", detail.process.id);
                let body =
                    format!("{}: {}", detail.process.goal, detail.process.status.as_str());
                match kind {
                    crate::notify::Kind::Review => crate::notify::review("processes", &title, &body),
                    crate::notify::Kind::Done => crate::notify::away("processes", &title, &body),
                }
            }
            state.detail = Some(*detail);
            state.error = None;
            // The scope only becomes addressable once the detail lands, so an
            // open panel gets its thread here rather than at Select.
            if state.chat_open {
                state.ensure_chat();
            }
            Task::none()
        }
        Message::Detailed(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::EventsLoaded(Ok(events)) => {
            state.events = events;
            Task::none()
        }
        Message::EventsLoaded(Err(_)) => Task::none(),
        Message::TeamsLoaded(Ok(teams)) => {
            state.lists_loaded.0 = true;
            state.error = None;
            if state.composer.team_id.is_none() {
                state.composer.team_id = teams.first().map(|t| t.id);
            }
            state.teams = teams;
            Task::none()
        }
        Message::TeamsLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::ProjectsLoaded(Ok(projects)) => {
            state.lists_loaded.1 = true;
            state.projects = projects;
            Task::none()
        }
        Message::ProjectsLoaded(Err(_)) => Task::none(),

        Message::GoalChanged(goal) => {
            state.composer.goal = goal;
            Task::none()
        }
        Message::TeamPicked(id) => {
            state.composer.team_id = Some(id);
            Task::none()
        }
        Message::ProjectPicked(id) => {
            state.composer.project_id = id;
            // Project doubles as the list scope, so the list must refetch.
            state.selected = None;
            state.detail = None;
            fetch_list(client, id)
        }
        Message::ToggleAutoApprove(v) => {
            state.composer.auto_approve = v;
            Task::none()
        }
        Message::Submit => {
            let (Some(team_id), goal) = (state.composer.team_id, state.composer.goal.trim())
            else {
                state.error = Some("Pick a team first.".into());
                return Task::none();
            };
            if goal.is_empty() {
                state.error = Some("Describe the goal first.".into());
                return Task::none();
            }
            state.composer.submitting = true;
            let body = CreateProcessBody {
                goal: goal.to_string(),
                team_template_id: team_id,
                auto_approve: Some(state.composer.auto_approve),
                project_id: state.composer.project_id,
            };
            let client = client.clone();
            Task::perform(
                async move { err_string(client.create_process(&body).await).map(|r| r.process_id) },
                Message::Created,
            )
        }
        Message::Created(Ok(id)) => {
            state.composer.submitting = false;
            state.composer.goal.clear();
            state.notice.set(format!("Started run #{id}."));
            update(state, client, Message::Select(id))
        }
        Message::Created(Err(e)) => {
            state.composer.submitting = false;
            state.error = Some(e);
            Task::none()
        }

        Message::SetView(view) => {
            state.view = view;
            Task::none()
        }
        Message::BoardSearchChanged(q) => {
            state.board_search = q;
            Task::none()
        }
        Message::ToggleNeedsAttention(v) => {
            state.needs_attention_only = v;
            Task::none()
        }
        Message::EventFilterChanged(f) => {
            state.event_filter = f;
            Task::none()
        }
        Message::Inspect(uuid) => {
            state.inspecting = uuid;
            if state.chat_open {
                state.ensure_chat();
            }
            Task::none()
        }
        Message::SetLineage(lineage) => {
            state.lineage = lineage;
            Task::none()
        }
        Message::Canvas(event) => {
            use crate::graph::CanvasEvent;
            match event {
                CanvasEvent::Selected(uuid) => state.inspecting = Some(uuid),
                CanvasEvent::Panned(delta) => state.viewport.pan(delta),
                CanvasEvent::Zoomed(delta) => state.viewport.zoom(delta),
            }
            Task::none()
        }

        Message::Approve => {
            let Some(id) = state.selected else { return Task::none() };
            let Some(dag_json) = state.selected_process().and_then(|p| p.dag_json.clone()) else {
                state.error = Some("This run has no plan to approve yet.".into());
                return Task::none();
            };
            // Validate locally first, same as the web client, so a malformed
            // plan reports field-level errors instead of a bare 422.
            match serde_json::from_str::<serde_json::Value>(&dag_json)
                .map_err(|e| vec![e.to_string()])
                .and_then(|v| agent_platform_client::dag::validate_planner_dag(&v))
            {
                Err(errors) => {
                    state.error = Some(format!("Plan is invalid: {}", errors.join("; ")));
                    Task::none()
                }
                Ok(_) => {
                    state.busy = true;
                    let client = client.clone();
                    Task::perform(
                        async move {
                            err_string(client.approve_process(id, &dag_json).await)
                                .map(|r| r.message.unwrap_or(r.status))
                        },
                        Message::ActionDone,
                    )
                }
            }
        }
        Message::Cancel => run_action(state, client, |c, id| async move {
            err_string(c.cancel_process(id).await).map(|r| r.status)
        }),
        Message::Retry => run_action(state, client, |c, id| async move {
            err_string(c.retry_process(id).await).map(|r| format!("retrying {}", r.retry.as_str()))
        }),
        Message::Sync => run_action(state, client, |c, id| async move {
            err_string(c.sync_process(id).await).map(|r| r.detail)
        }),
        Message::Export => run_action(state, client, export_process),
        Message::RetryTask(task_id) => run_action(state, client, move |c, id| async move {
            err_string(c.retry_task(id, task_id).await).map(|r| format!("task {} requeued", r.task_id))
        }),

        Message::OpenReview(task_id) => {
            let task = state
                .detail
                .as_ref()
                .and_then(|d| d.tasks.iter().find(|t| t.id == task_id));
            state.review = task.map(|t| ReviewDraft {
                task_id,
                role: t.role.clone(),
                // Reviewers edit the draft; fall back to final output.
                output: t.draft_output.clone().or_else(|| t.output.clone()).unwrap_or_default(),
                feedback: String::new(),
                instructions: String::new(),
            });
            Task::none()
        }
        Message::CloseReview => {
            state.review = None;
            Task::none()
        }
        Message::ReviewOutputChanged(v) => {
            if let Some(r) = &mut state.review {
                r.output = v;
            }
            Task::none()
        }
        Message::ReviewFeedbackChanged(v) => {
            if let Some(r) = &mut state.review {
                r.feedback = v;
            }
            Task::none()
        }
        Message::ReviewInstructionsChanged(v) => {
            if let Some(r) = &mut state.review {
                r.instructions = v;
            }
            Task::none()
        }
        Message::SubmitReview(decision) => {
            let (Some(id), Some(draft)) = (state.selected, state.review.take()) else {
                return Task::none();
            };
            let body = ReviewTaskBody {
                decision,
                output: (decision == ReviewDecision::Approve).then(|| draft.output.clone()),
                feedback: non_empty(&draft.feedback),
                instructions: non_empty(&draft.instructions),
            };
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move {
                    err_string(client.review_task(id, draft.task_id, &body).await)
                        .map(|r| r.message.unwrap_or(r.status))
                },
                Message::ActionDone,
            )
        }
        Message::ActionDone(result) => {
            state.busy = false;
            match result {
                Ok(msg) => state.notice.set(msg),
                Err(e) => state.error = Some(e),
            }
            match state.selected {
                Some(id) => fetch_detail(client, id),
                None => Task::none(),
            }
        }
        Message::DismissNotice => {
            state.notice.clear();
            state.error = None;
            Task::none()
        }
        Message::ToggleChat => {
            state.chat_open = !state.chat_open;
            if state.chat_open {
                state.ensure_chat();
            }
            Task::none()
        }
        Message::Chat(msg) => {
            let Some(key) = state.chat_key() else { return Task::none() };
            let system = state.scope_system();
            let (provider, model) = state.chat_default.clone();
            let thread =
                state.chats.entry(key.clone()).or_insert_with(|| crate::chat::State::scoped(&key));
            thread.system = system;
            crate::chat::update(thread, client, (&provider, &model), msg).map(Message::Chat)
        }
    }
}

fn is_terminal_status(status: ProcessStatus) -> bool {
    matches!(status, ProcessStatus::Completed | ProcessStatus::Failed | ProcessStatus::Cancelled)
}

/// The run stopped moving on its own and is waiting for a human — the two
/// states where the engine will not take another step until someone answers.
fn needs_user(status: ProcessStatus) -> bool {
    matches!(status, ProcessStatus::ApprovalRequired | ProcessStatus::TaskReviewRequired)
}

/// What to announce, on the one poll where the run first stops moving: `Done`
/// when it ended, `Review` when it is blocked on the user.
///
/// `None` while it is still running, on every later poll of the same status
/// (a finished run would otherwise re-notify every four seconds), and when
/// there is no previous status at all — that is the detail landing for a run
/// the user just picked, which is not news about anything.
fn settled(
    previous: Option<ProcessStatus>,
    current: ProcessStatus,
) -> Option<crate::notify::Kind> {
    let resting = is_terminal_status(current) || needs_user(current);
    if !resting || previous.is_none_or(|p| p == current) {
        return None;
    }
    Some(match needs_user(current) {
        true => crate::notify::Kind::Review,
        false => crate::notify::Kind::Done,
    })
}

/// Run a process-scoped action against the selected run.
/// Writes `{exported_at, process, tasks, events}` to a file the user picks —
/// the whole run, not the page currently on screen, so events are walked to the
/// end. Cancelling the picker is a no-op, not an error.
async fn export_process(client: Client, id: i64) -> Result<String, String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_title("Export run")
        .set_file_name(format!("run-{id}.json"))
        .add_filter("JSON", &["json"])
        .save_file()
        .await
    else {
        return Ok("Export cancelled.".into());
    };

    let detail = client.process_detail(id).await.map_err(|e| e.to_string())?;
    let events = client.all_process_events(id).await.map_err(|e| e.to_string())?;
    let count = events.len();
    let payload = serde_json::json!({
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "process": detail.process,
        "tasks": detail.tasks,
        "events": events,
    });
    let bytes = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
    let name = handle.file_name();
    handle.write(&bytes).await.map_err(|e| e.to_string())?;
    Ok(format!("Exported {count} event(s) to {name}."))
}

fn run_action<F, Fut>(state: &mut State, client: &Client, f: F) -> Task<Message>
where
    F: FnOnce(Client, i64) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
{
    let Some(id) = state.selected else { return Task::none() };
    state.busy = true;
    Task::perform(f(client.clone(), id), Message::ActionDone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn detail(status: &str, dag: bool) -> ProcessDetailResponse {
        serde_json::from_value(json!({
            "process": {
                "id": 1, "goal": "g", "status": status,
                "dag_json": dag.then(|| json!({
                    "team_name": "t", "goal_restatement": "g",
                    "subagents": [{"client_uuid":"a","role":"R","system_prompt":"s","instructions":"i"}]
                }).to_string()),
                "failure_reason": null, "total_tokens": 0, "total_cost": 0.0,
                "created_at": "2026-08-02T10:00:00", "updated_at": "2026-08-02T10:00:00"
            },
            "tasks": []
        }))
        .unwrap()
    }

    #[test]
    fn live_states_poll_fast_and_stream() {
        let mut s = State::default();
        for status in ["pending", "planning", "approved", "running"] {
            s.detail = Some(detail(status, false));
            assert!(s.is_live(), "{status}");
            assert!(s.stream_eligible(), "{status}");
            assert_eq!(s.detail_poll_interval(), Duration::from_millis(800));
        }
    }

    #[test]
    fn gated_and_terminal_states_do_not_stream() {
        let mut s = State::default();
        for status in ["approval_required", "task_review_required", "completed", "failed", "cancelled"] {
            s.detail = Some(detail(status, false));
            assert!(!s.stream_eligible(), "{status}");
            assert_eq!(s.detail_poll_interval(), Duration::from_secs(4));
        }
    }

    #[test]
    fn board_filters_apply() {
        let mut s = State::default();
        s.detail = Some(detail("running", true));
        assert_eq!(s.board_rows().len(), 1);
        s.board_search = "zzz".into();
        assert!(s.board_rows().is_empty());
        s.board_search = "R".into();
        assert_eq!(s.board_rows().len(), 1);
        // Pending row only counts as "needs attention" while approval is pending.
        s.board_search.clear();
        s.needs_attention_only = true;
        assert!(s.board_rows().is_empty());
        s.detail = Some(detail("approval_required", true));
        assert_eq!(s.board_rows().len(), 1);
    }

    #[test]
    fn approve_rejects_a_run_with_no_plan() {
        let mut s = State::default();
        s.selected = Some(1);
        s.detail = Some(detail("approval_required", false));
        let client = Client::new("http://127.0.0.1:1", "k");
        let _ = update(&mut s, &client, Message::Approve);
        assert!(s.error.as_deref().unwrap().contains("no plan"));
        assert!(!s.busy);
    }

    #[test]
    fn notification_fires_once_when_the_run_stops_moving() {
        use crate::notify::Kind;
        use ProcessStatus::*;
        // Still moving, or the first detail of a run just selected: not news.
        assert_eq!(settled(Some(Running), Running), None);
        assert_eq!(settled(None, Completed), None);
        // Ended.
        assert_eq!(settled(Some(Running), Failed), Some(Kind::Done));
        assert_eq!(settled(Some(Approved), Completed), Some(Kind::Done));
        // Blocked on the user — the one worth interrupting for.
        assert_eq!(settled(Some(Planning), ApprovalRequired), Some(Kind::Review));
        assert_eq!(settled(Some(Running), TaskReviewRequired), Some(Kind::Review));
        // Answering the approval and finishing later still announces the end.
        assert_eq!(settled(Some(ApprovalRequired), Completed), Some(Kind::Done));
        // Re-polling the same resting status does not re-fire.
        assert_eq!(settled(Some(Completed), Completed), None);
        assert_eq!(settled(Some(ApprovalRequired), ApprovalRequired), None);
    }

    #[test]
    fn chat_scope_follows_the_inspector() {
        let mut s = State::default();
        s.detail = Some(detail("running", true));

        assert_eq!(s.chat_key().as_deref(), Some("1"));
        let run_scope = s.scope_system().unwrap();
        assert!(run_scope.contains("Process id: 1"));
        assert!(run_scope.contains("Goal: g"));
        assert!(!run_scope.contains("client_uuid"));

        s.inspecting = Some("a".into());
        assert_eq!(s.chat_key().as_deref(), Some("1:a"));
        let sub_scope = s.scope_system().unwrap();
        assert!(sub_scope.contains("Focused client_uuid: a"));
        assert!(sub_scope.contains("Role: R"));

        // Each scope owns its own thread rather than sharing one.
        s.ensure_chat();
        s.inspecting = None;
        s.ensure_chat();
        assert_eq!(s.chats.len(), 2);
    }

    #[test]
    fn teams_recovering_after_the_daemon_finishes_booting_clears_the_banner() {
        let mut s = State::default();
        let client = Client::new("http://127.0.0.1:1", "k");
        let _ = update(&mut s, &client, Message::TeamsLoaded(Err("connection refused".into())));
        assert!(s.error.is_some());
        let _ = update(&mut s, &client, Message::TeamsLoaded(Ok(vec![])));
        assert!(s.error.is_none());
    }

    #[test]
    fn submit_requires_goal_and_team() {
        let mut s = State::default();
        let client = Client::new("http://127.0.0.1:1", "k");
        let _ = update(&mut s, &client, Message::Submit);
        assert!(s.error.as_deref().unwrap().contains("team"));
        s.composer.team_id = Some(7);
        s.error = None;
        let _ = update(&mut s, &client, Message::Submit);
        assert!(s.error.as_deref().unwrap().contains("goal"));
        assert!(!s.composer.submitting);
    }
}
