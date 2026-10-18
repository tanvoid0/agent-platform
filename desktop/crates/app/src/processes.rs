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
use iced::widget::{markdown, text_editor};
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

/// Run-list scope. A long-lived install accumulates runs, and the list was
/// every one of them with no way to reach the live ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunScope {
    All,
    Live,
    Finished,
}

impl RunScope {
    pub const ALL: [RunScope; 3] = [RunScope::All, RunScope::Live, RunScope::Finished];

    pub fn label(self) -> &'static str {
        match self {
            RunScope::All => "All",
            RunScope::Live => "Live",
            RunScope::Finished => "Finished",
        }
    }

    fn matches(self, status: &str) -> bool {
        let terminal = matches!(status, "completed" | "failed" | "cancelled");
        match self {
            RunScope::All => true,
            RunScope::Live => !terminal,
            RunScope::Finished => terminal,
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
    /// Rendered markdown for the *open* event only, keyed by its id. Every
    /// event was parsed on every load before this, and a live run reloads all
    /// 2000 of them every 800ms — only the one on screen is ever rendered.
    pub event_md: Option<(i64, Vec<markdown::Item>)>,
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
    /// Event id whose full body is open in the events sidebar.
    pub event_open: Option<i64>,
    /// Events dropped off the front of the buffer, so the tab can say so.
    pub events_trimmed: usize,
    /// Filter over the run list — matches the goal text or `#id`.
    pub run_search: String,
    /// Which runs the list shows: all, live, or finished.
    pub run_scope: RunScope,
    /// Board columns folded away, so a run with fifty done tasks still fits.
    pub collapsed: std::collections::HashSet<BoardColumn>,
    /// A reject waiting on the confirm dialog — it fails the whole run.
    pub confirm_reject: Option<i64>,
    /// Rendered markdown for the inspected task's output, keyed by task id and
    /// output length. Only the open task is parsed, and only when it changes —
    /// a detail poll lands every 800ms and outputs run to thousands of lines.
    pub output_md: Option<(i64, usize, Vec<markdown::Item>)>,
    pub review: Option<ReviewDraft>,
    /// Multi-line editor for [`ReviewDraft::output`]. Not on the draft: iced's
    /// `Content` is not `Clone`, and review messages still need to be.
    pub review_output: text_editor::Content,
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
            event_md: None,
            teams: Vec::new(),
            projects: Vec::new(),
            lists_loaded: (false, false),
            composer: Composer::default(),
            view: ViewMode::Graph,
            board_search: String::new(),
            needs_attention_only: false,
            event_filter: String::new(),
            inspecting: None,
            event_open: None,
            events_trimmed: 0,
            run_search: String::new(),
            run_scope: RunScope::All,
            collapsed: Default::default(),
            confirm_reject: None,
            output_md: None,
            review: None,
            review_output: text_editor::Content::new(),
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

    /// The runs the list shows under the current search and scope.
    pub fn visible_processes(&self) -> Vec<&ProcessRecord> {
        let needle = self.run_search.trim().to_lowercase();
        self.processes
            .iter()
            .filter(|p| self.run_scope.matches(p.status.as_str()))
            .filter(|p| {
                needle.is_empty()
                    || p.goal.to_lowercase().contains(&needle)
                    || p.id.to_string() == needle.trim_start_matches('#')
            })
            .collect()
    }

    /// The highest event id held, which is what the next page asks after.
    pub fn event_cursor(&self) -> i64 {
        self.events.last().map(|e| e.id).unwrap_or(0)
    }

    /// Parse the open event's body, once. Cheap when nothing moved: an event is
    /// immutable, so its id alone is the key.
    pub fn refresh_event_md(&mut self) {
        let Some(id) = self.event_open else {
            self.event_md = None;
            return;
        };
        if self.event_md.as_ref().map(|(cached, _)| *cached) == Some(id) {
            return;
        }
        self.event_md = self
            .events
            .iter()
            .find(|e| e.id == id)
            .map(|e| (id, markdown::parse(&e.content).collect()));
    }

    /// Reparse the inspected task's output if it is new to us. Cheap when
    /// nothing moved: the (id, len) key short-circuits.
    pub fn refresh_output_md(&mut self) {
        let Some(uuid) = self.inspecting.clone() else {
            self.output_md = None;
            return;
        };
        let Some(task) = self.task_by_uuid(&uuid) else {
            self.output_md = None;
            return;
        };
        let Some(output) = task.output.as_deref().or(task.draft_output.as_deref()) else {
            self.output_md = None;
            return;
        };
        let key = (task.id, output.len());
        if self.output_md.as_ref().map(|(id, len, _)| (*id, *len)) == Some(key) {
            return;
        }
        self.output_md = Some((key.0, key.1, markdown::parse(output).collect()));
    }

    /// Task ids sitting at the review gate. Read off the task list, not the
    /// board rows — a board search must not narrow what "Approve all" approves.
    pub fn awaiting_review_task_ids(&self) -> Vec<i64> {
        self.detail
            .as_ref()
            .map(|d| {
                d.tasks
                    .iter()
                    .filter(|t| t.status == "awaiting_review")
                    .map(|t| t.id)
                    .collect()
            })
            .unwrap_or_default()
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
    /// A link in an event's rendered markdown.
    LinkClicked(String),
    /// Open (or close, with `None`) an event's full body in the sidebar.
    OpenEvent(Option<i64>),
    /// Put one event's full body on the clipboard.
    CopyEvent(i64),
    /// Put a task's output on the clipboard.
    CopyOutput(i64),
    /// Jump from an event to the subagent whose task raised it.
    InspectTask(i64),
    /// Recentre the graph canvas after a pan or zoom loses the nodes.
    ResetViewport,
    /// The settings link in this screen's header — the key a failed run needs lives in Settings.
    /// Intercepted in `main::update` the same way, so this arm exists
    /// only to satisfy exhaustiveness.
    OpenSettings,
    /// The composer's way out when no team exists yet. Intercepted in
    /// `main::update` like the rest; a run cannot start without one.
    OpenTeams,
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
    /// Flip auto-approve on the *selected, already-created* process, unlike
    /// [`Message::ToggleAutoApprove`], which sets it on the composer.
    SetAutoApprove(bool),
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
    /// Approve a task straight from the board, keeping the agent's own output.
    ApproveTask(i64),
    /// Approve every task waiting at the review gate, in one pass.
    ApproveAllReviews,
    /// Ask before rejecting — a reject fails the run, so it is not one click.
    RejectTask(i64),
    /// Yes, reject it.
    RejectTaskConfirmed(i64),
    /// Dismiss the reject confirmation.
    CancelReject,
    /// Fold a board column away, or unfold it.
    ToggleColumn(BoardColumn),
    RunSearchChanged(String),
    SetRunScope(RunScope),
    CloseReview,
    ReviewOutputEdited(text_editor::Action),
    ReviewFeedbackChanged(String),
    ReviewInstructionsChanged(String),
    SubmitReview(ReviewDecision),
    ActionDone(Result<String, String>),
    /// Sync's own completion: it answers 200 even on the branches where it
    /// declines to act, so the outcome is in the body, not the status code.
    SyncDone(Result<SyncProcessResponse, String>),
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

/// One page of events. The route is a *forward* cursor (`id > after_id`, ASC,
/// server-capped at 2000), so asking from 0 every poll returned the run's
/// oldest 2000 for ever — a long run never showed its latest event at all.
const EVENT_PAGE: u32 = 2000;

/// How many events are kept in memory. A run that outruns this loses its
/// oldest, which the events tab says out loud; Export still walks them all.
const EVENT_BUFFER: usize = 5000;

fn fetch_events(client: &Client, id: i64, after_id: i64) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            err_string(client.process_events(id, None, EVENT_PAGE, after_id).await)
                .map(|r| r.events)
        },
        Message::EventsLoaded,
    )
}

fn fetch_detail(client: &Client, id: i64, after_id: i64) -> Task<Message> {
    let c1 = client.clone();
    Task::batch([
        Task::perform(
            async move { err_string(c1.process_detail(id).await).map(Box::new) },
            Message::Detailed,
        ),
        fetch_events(client, id, after_id),
    ])
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) | Message::OpenSettings | Message::OpenTeams => Task::none(),
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
            state.events_trimmed = 0;
            state.event_md = None;
            // A filter and an open event belong to the run they were opened on.
            state.event_filter.clear();
            state.inspecting = None;
            state.event_open = None;
            close_review(state);
            state.viewport = crate::graph::Viewport::default();
            fetch_detail(client, id, 0)
        }
        Message::DetailTick | Message::StreamFrame => match state.selected {
            Some(id) => fetch_detail(client, id, state.event_cursor()),
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
            state.refresh_output_md();
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
            let full_page = events.len() >= EVENT_PAGE as usize;
            let cursor = state.event_cursor();
            state.events.extend(events.into_iter().filter(|e| e.id > cursor));
            let overflow = state.events.len().saturating_sub(EVENT_BUFFER);
            if overflow > 0 {
                state.events.drain(..overflow);
                state.events_trimmed += overflow;
            }
            state.refresh_event_md();
            // A full page means the cursor has not caught up yet — keep draining
            // rather than waiting a poll per page on a run with a long backlog.
            // Gated on the cursor actually moving: a full page of ids we already
            // hold would otherwise refetch itself for ever.
            match (full_page && state.event_cursor() > cursor, state.selected) {
                (true, Some(id)) => fetch_events(client, id, state.event_cursor()),
                _ => Task::none(),
            }
        }
        Message::EventsLoaded(Err(_)) => Task::none(),
        Message::CopyEvent(id) => {
            let Some(event) = state.events.iter().find(|e| e.id == id) else {
                return Task::none();
            };
            state.notice.set("Event copied.");
            iced::clipboard::write(event.content.clone())
        }
        Message::CopyOutput(task_id) => {
            let text = state
                .detail
                .as_ref()
                .and_then(|d| d.tasks.iter().find(|t| t.id == task_id))
                .and_then(|t| t.output.clone().or_else(|| t.draft_output.clone()));
            match text {
                Some(text) => {
                    state.notice.set("Output copied.");
                    iced::clipboard::write(text)
                }
                None => Task::none(),
            }
        }
        Message::InspectTask(task_id) => {
            // The event only carries a task id; the board and inspector are
            // keyed by client_uuid, so a jump has to go through the task list.
            let Some(task) = state.detail.as_ref().and_then(|d| d.tasks.iter().find(|t| t.id == task_id))
            else {
                state.notice.set("That task is not in this run's plan any more.");
                return Task::none();
            };
            state.inspecting = Some(task.client_uuid.clone());
            state.view = ViewMode::Board;
            state.refresh_output_md();
            if state.chat_open {
                state.ensure_chat();
            }
            Task::none()
        }
        Message::ResetViewport => {
            state.viewport = crate::graph::Viewport::default();
            Task::none()
        }
        Message::OpenEvent(id) => {
            state.event_open = id;
            state.refresh_event_md();
            Task::none()
        }
        Message::LinkClicked(url) => {
            if url.starts_with("http://") || url.starts_with("https://") {
                crate::shell::reveal_path(&url);
            }
            Task::none()
        }
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
            // Keep the sidebar honest: an event the filter hides cannot stay open.
            if let Some(open) = state.event_open {
                let needle = state.event_filter.to_lowercase();
                let still_shown = state.events.iter().any(|e| {
                    e.id == open
                        && (needle.is_empty()
                            || e.event_type.to_lowercase().contains(&needle)
                            || e.content.to_lowercase().contains(&needle))
                });
                if !still_shown {
                    state.event_open = None;
                    state.event_md = None;
                }
            }
            Task::none()
        }
        Message::Inspect(uuid) => {
            state.inspecting = uuid;
            state.refresh_output_md();
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
                CanvasEvent::Selected(uuid) => {
                    state.inspecting = Some(uuid);
                    state.refresh_output_md();
                }
                CanvasEvent::Panned(delta) => state.viewport.pan(delta),
                CanvasEvent::Zoomed(delta) => state.viewport.zoom(delta),
            }
            Task::none()
        }

        Message::SetAutoApprove(on) => run_action(state, client, move |c, id| async move {
            err_string(c.set_process_auto_approve(id, on).await)
                .map(|_| if on { "Auto-approve on".to_string() } else { "Auto-approve off".to_string() })
        }),

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
        Message::Sync => {
            let Some(id) = state.selected else { return Task::none() };
            state.busy = true;
            let c = client.clone();
            Task::perform(async move { err_string(c.sync_process(id).await) }, Message::SyncDone)
        }
        Message::Export => run_action(state, client, export_process),
        Message::RetryTask(task_id) => run_action(state, client, move |c, id| async move {
            err_string(c.retry_task(id, task_id).await).map(|r| format!("task {} requeued", r.task_id))
        }),

        Message::ApproveTask(task_id) => {
            let Some(id) = state.selected else { return Task::none() };
            state.busy = true;
            approve_all(client.clone(), id, vec![task_id])
        }
        Message::ApproveAllReviews => {
            let Some(id) = state.selected else { return Task::none() };
            let ids = state.awaiting_review_task_ids();
            if ids.is_empty() {
                state.notice.set("Nothing is waiting for review.");
                return Task::none();
            }
            state.busy = true;
            approve_all(client.clone(), id, ids)
        }
        Message::RejectTask(task_id) => {
            state.confirm_reject = Some(task_id);
            Task::none()
        }
        Message::CancelReject => {
            state.confirm_reject = None;
            Task::none()
        }
        Message::RejectTaskConfirmed(task_id) => {
            state.confirm_reject = None;
            // The reject may have been raised from inside the review modal.
            close_review(state);
            let Some(id) = state.selected else { return Task::none() };
            state.busy = true;
            let client = client.clone();
            let body = ReviewTaskBody {
                decision: ReviewDecision::Reject,
                output: None,
                feedback: None,
                instructions: None,
            };
            Task::perform(
                async move {
                    err_string(client.review_task(id, task_id, &body).await)
                        .map(|r| r.message.unwrap_or(r.status))
                },
                Message::ActionDone,
            )
        }
        Message::RunSearchChanged(v) => {
            state.run_search = v;
            Task::none()
        }
        Message::SetRunScope(scope) => {
            state.run_scope = scope;
            Task::none()
        }
        Message::ToggleColumn(column) => {
            if !state.collapsed.remove(&column) {
                state.collapsed.insert(column);
            }
            Task::none()
        }
        Message::OpenReview(task_id) => {
            let task = state
                .detail
                .as_ref()
                .and_then(|d| d.tasks.iter().find(|t| t.id == task_id));
            state.review = task.map(|t| ReviewDraft {
                task_id,
                role: t.role.clone(),
                output: t.draft_output.clone().or_else(|| t.output.clone()).unwrap_or_default(),
                feedback: String::new(),
                instructions: String::new(),
            });
            state.review_output = text_editor::Content::with_text(
                state.review.as_ref().map(|r| r.output.as_str()).unwrap_or(""),
            );
            Task::none()
        }
        Message::CloseReview => {
            close_review(state);
            Task::none()
        }
        Message::ReviewOutputEdited(action) => {
            state.review_output.perform(action);
            if let Some(r) = &mut state.review {
                r.output = state.review_output.text();
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
            if let Some(r) = &mut state.review {
                r.output = state.review_output.text();
            }
            let (Some(id), Some(draft)) = (state.selected, state.review.take()) else {
                return Task::none();
            };
            close_review(state);
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
            match result {
                Ok(msg) => state.notice.set(msg),
                Err(e) => state.error = Some(e),
            }
            after_action(state, client)
        }
        Message::SyncDone(result) => {
            match result {
                // `none` is a terminal run, `blocked` is an open gate. The server
                // took the call and ran nothing, so a green tick is a lie — the
                // detail says what to press instead, and it has to read that way.
                Ok(r) if sync_acted(&r.action) => state.notice.set(r.detail),
                Ok(r) => state.notice.set_info(r.detail),
                Err(e) => state.error = Some(e),
            }
            after_action(state, client)
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

fn close_review(state: &mut State) {
    state.review = None;
    state.review_output = text_editor::Content::new();
}

fn is_terminal_status(status: ProcessStatus) -> bool {
    matches!(status, ProcessStatus::Completed | ProcessStatus::Failed | ProcessStatus::Cancelled)
}

/// The run has not finished: it is planning, executing, or stopped for a human.
/// Home lists these, because a run the user just started is one of them and
/// showed up nowhere until it either failed or asked a question.
pub(crate) fn is_live(status: ProcessStatus) -> bool {
    !is_terminal_status(status) && status != ProcessStatus::Unknown
}

/// The run stopped moving on its own and is waiting for a human — the two
/// states where the engine will not take another step until someone answers.
pub(crate) fn needs_user(status: ProcessStatus) -> bool {
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
/// Approve tasks without opening the review modal — the agent's own output is
/// kept (an omitted `output` means "keep what it produced"). Sequential, because
/// each approval can expand into a sub-DAG the next wave is computed from.
fn approve_all(client: Client, process_id: i64, task_ids: Vec<i64>) -> Task<Message> {
    Task::perform(
        async move {
            let total = task_ids.len();
            let mut failed: Vec<String> = Vec::new();
            for task_id in task_ids {
                let body = ReviewTaskBody {
                    decision: ReviewDecision::Approve,
                    output: None,
                    feedback: None,
                    instructions: None,
                };
                if let Err(e) = client.review_task(process_id, task_id, &body).await {
                    failed.push(format!("task {task_id}: {e}"));
                }
            }
            match failed.len() {
                0 => Ok(format!("Approved {}.", crate::ui::count(total, "task", "tasks"))),
                n if n == total => Err(failed.join("; ")),
                n => Ok(format!("Approved {} of {total}; {n} failed: {}", total - n, failed.join("; "))),
            }
        },
        Message::ActionDone,
    )
}

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

/// Did sync actually do something? `none` is a terminal run and `blocked` is an
/// open gate — both answer 200 having run nothing. `Unknown` is a server newer
/// than this build; assume it acted, since the alternative mislabels real work.
fn sync_acted(action: &ProcessSyncAction) -> bool {
    !matches!(action, ProcessSyncAction::None | ProcessSyncAction::Blocked)
}

/// Every action's tail: the request is over, and the row it touched is stale.
fn after_action(state: &mut State, client: &Client) -> Task<Message> {
    state.busy = false;
    match state.selected {
        Some(id) => fetch_detail(client, id, state.event_cursor()),
        None => Task::none(),
    }
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

    /// Approve-all reads the review gate, not the whole task list, and says so
    /// rather than firing an empty batch.
    #[test]
    fn approve_all_only_takes_tasks_at_the_review_gate() {
        let mut s = State::default();
        s.selected = Some(1);
        let mut d = detail("running", true);
        d.tasks = serde_json::from_value(json!([
            {"id": 7, "process_id": 1, "client_uuid": "a", "role": "R", "system_prompt": "s",
             "instructions": "i", "llm_model": null, "dependencies_json": "[]",
             "status": "awaiting_review", "output": null, "tokens_used": 0,
             "started_at": null, "completed_at": null},
            {"id": 8, "process_id": 1, "client_uuid": "b", "role": "R2", "system_prompt": "s",
             "instructions": "i", "llm_model": null, "dependencies_json": "[]",
             "status": "running", "output": null, "tokens_used": 0,
             "started_at": null, "completed_at": null}
        ]))
        .unwrap();
        s.detail = Some(d);
        assert_eq!(s.awaiting_review_task_ids(), vec![7]);
        // A board filter narrows the board, never the batch.
        s.board_search = "zzz".into();
        assert_eq!(s.awaiting_review_task_ids(), vec![7]);
        s.board_search.clear();

        s.detail = Some(detail("running", true));
        let client = Client::new("http://127.0.0.1:1", "k");
        let _ = update(&mut s, &client, Message::ApproveAllReviews);
        assert!(!s.busy);
        assert!(s.notice.get().unwrap().0.contains("Nothing is waiting"));
    }

    /// Sync answers 200 on the branches where it declines to act. A green tick
    /// there reads as "done" on a run where nothing ran.
    #[test]
    fn a_sync_that_ran_nothing_does_not_read_as_success() {
        for declined in [ProcessSyncAction::None, ProcessSyncAction::Blocked] {
            assert!(!sync_acted(&declined), "{declined:?} ran nothing");
        }
        for acted in [
            ProcessSyncAction::AlignedStatus,
            ProcessSyncAction::RequeuedPlan,
            ProcessSyncAction::RequeuedExecution,
            // A server newer than this build: assume it did the work.
            ProcessSyncAction::Unknown,
        ] {
            assert!(sync_acted(&acted), "{acted:?} did something");
        }

        let mut t = crate::domain::Toast::default();
        t.set_info("blocked");
        assert_eq!(t.get().unwrap().2, crate::ui::Tone::Info);
        t.set("requeued");
        assert_eq!(t.get().unwrap().2, crate::ui::Tone::Success);
    }

    /// The list narrows by scope and text, and `#id` finds a run by number.
    #[test]
    fn the_run_list_filters_by_scope_and_text() {
        let mut s = State::default();
        s.processes = serde_json::from_value(json!([
            {"id": 1, "goal": "ship the docs", "status": "running", "total_tokens": 0,
             "total_cost": 0.0, "created_at": "2026-08-02T10:00:00", "updated_at": "2026-08-02T10:00:00"},
            {"id": 2, "goal": "fix the parser", "status": "completed", "total_tokens": 0,
             "total_cost": 0.0, "created_at": "2026-08-02T10:00:00", "updated_at": "2026-08-02T10:00:00"}
        ]))
        .unwrap();
        assert_eq!(s.visible_processes().len(), 2);
        s.run_scope = RunScope::Live;
        assert_eq!(s.visible_processes().iter().map(|p| p.id).collect::<Vec<_>>(), vec![1]);
        s.run_scope = RunScope::Finished;
        assert_eq!(s.visible_processes().iter().map(|p| p.id).collect::<Vec<_>>(), vec![2]);
        s.run_scope = RunScope::All;
        s.run_search = "parser".into();
        assert_eq!(s.visible_processes().len(), 1);
        s.run_search = "#1".into();
        assert_eq!(s.visible_processes().iter().map(|p| p.id).collect::<Vec<_>>(), vec![1]);
    }

    /// The event feed is a forward cursor: pages append, the cursor advances,
    /// and nothing already held is refetched.
    #[test]
    fn events_append_from_the_cursor_and_never_refetch() {
        fn page(ids: &[i64]) -> Vec<EventLogRecord> {
            serde_json::from_value(json!(ids
                .iter()
                .map(|id| json!({
                    "id": id, "process_id": 1, "task_id": null, "event_type": "trace",
                    "content": "x", "created_at": "2026-08-02T10:00:00"
                }))
                .collect::<Vec<_>>()))
            .unwrap()
        }
        let mut s = State::default();
        let client = Client::new("http://127.0.0.1:1", "k");
        assert_eq!(s.event_cursor(), 0);
        let _ = update(&mut s, &client, Message::EventsLoaded(Ok(page(&[1, 2, 3]))));
        assert_eq!(s.event_cursor(), 3);
        // The server answers `id > after_id`, but a replayed page must not
        // duplicate rows if one ever arrives.
        let _ = update(&mut s, &client, Message::EventsLoaded(Ok(page(&[2, 3, 4]))));
        assert_eq!(s.events.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(s.event_cursor(), 4);
        assert_eq!(s.events_trimmed, 0);
    }

    /// An event the filter hides must not stay open in the sidebar.
    #[test]
    fn filtering_closes_an_event_it_hides() {
        let mut s = State::default();
        let client = Client::new("http://127.0.0.1:1", "k");
        let events: Vec<EventLogRecord> = serde_json::from_value(json!([
            {"id": 5, "process_id": 1, "task_id": null, "event_type": "trace",
             "content": "planner said hello", "created_at": "2026-08-02T10:00:00"}
        ]))
        .unwrap();
        let _ = update(&mut s, &client, Message::EventsLoaded(Ok(events)));
        assert!(s.event_md.is_none(), "nothing is parsed until an event is opened");
        let _ = update(&mut s, &client, Message::OpenEvent(Some(5)));
        assert_eq!(s.event_md.as_ref().map(|(id, _)| *id), Some(5));
        let _ = update(&mut s, &client, Message::EventFilterChanged("trace".into()));
        assert_eq!(s.event_open, Some(5), "a filter it matches leaves it open");
        let _ = update(&mut s, &client, Message::EventFilterChanged("error".into()));
        assert!(s.event_open.is_none());
    }

    /// Reject ends the run, so it asks first and only fires on the confirm.
    #[test]
    fn reject_asks_before_it_ends_the_run() {
        let mut s = State::default();
        s.selected = Some(1);
        s.detail = Some(detail("running", true));
        let client = Client::new("http://127.0.0.1:1", "k");
        let _ = update(&mut s, &client, Message::RejectTask(7));
        assert_eq!(s.confirm_reject, Some(7));
        assert!(!s.busy, "nothing is sent until the dialog is answered");
        let _ = update(&mut s, &client, Message::CancelReject);
        assert!(s.confirm_reject.is_none());
        let _ = update(&mut s, &client, Message::RejectTask(7));
        let _ = update(&mut s, &client, Message::RejectTaskConfirmed(7));
        assert!(s.confirm_reject.is_none());
        assert!(s.busy);
    }

    /// A folded column is a view state, not a filter: the rows stay.
    #[test]
    fn a_board_column_folds_and_unfolds() {
        let mut s = State::default();
        s.detail = Some(detail("running", true));
        let client = Client::new("http://127.0.0.1:1", "k");
        let _ = update(&mut s, &client, Message::ToggleColumn(BoardColumn::Pending));
        assert!(s.collapsed.contains(&BoardColumn::Pending));
        assert_eq!(s.rows_in_column(BoardColumn::Pending).len(), 1);
        let _ = update(&mut s, &client, Message::ToggleColumn(BoardColumn::Pending));
        assert!(s.collapsed.is_empty());
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
