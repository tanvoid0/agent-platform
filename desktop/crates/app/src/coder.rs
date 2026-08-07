//! Coder — coding against the open folder, with the model doing the typing.
//!
//! The agent loop is the server's (`app/coder/service.py`): it owns the thread,
//! the tool specs, the iteration cap and the approval gate, and it reaches every
//! provider the proxy does. What this screen adds is the half that has to be
//! native — the tools run *here*, on the user's real files, through
//! [`crate::coder_tools`].
//!
//! The wiring for that already existed server-side and had no client:
//! `delegate_tools` swaps the executor for one that emits the call and parks the
//! turn on a future keyed `(thread_id, call_id)`. So a turn reads:
//!
//! ```text
//! POST /coder/chat/stream ─→ event: tool_call {call_id, name, arguments}
//!                            (server blocked, 300s)
//!         run it here ──────→ POST /coder/chat/tool-result
//!                          ←─ event: tool_result, then the next step
//! ```
//!
//! Answering is not optional: a dropped `tool_call` stalls the turn until the
//! server's timeout, so every path out of [`Message::Event`] posts something,
//! including the failures.
//!
//! One thread per session, not the saved-conversation sidebar the chat screens
//! have — the server persists coder threads already, and listing them is a
//! screen of its own rather than a fourth thing in this one.

use agent_platform_client::sse::{coder_stream, CoderEvent};
use agent_platform_client::types::{CoderThreadSummary, ProviderEntry};
use agent_platform_client::Client;
use iced::widget::markdown;
use iced::Task;
use std::path::PathBuf;

/// Identity of the transcript scrollable, so a reply can snap it to the end.
pub fn transcript_id() -> iced::widget::Id {
    iced::widget::Id::new("coder-transcript")
}

/// One row of the transcript. Tool calls are rows rather than hidden plumbing:
/// the user reading them is the only check on what the agent touched, and with
/// `run_command` behind an approval gate it is also how they decide.
#[derive(Debug)]
pub enum Turn {
    User(String),
    Assistant { text: String, md: Vec<markdown::Item> },
    Tool {
        /// `read_file src/a.rs`, `$ cargo test` — what was done, not which
        /// function did it.
        label: String,
        /// `None` while it runs. `Ok` is the model's own view of the outcome;
        /// `Err` is a call that never produced one — refused, or abandoned when
        /// its turn died. Kept apart because both render as text and a row that
        /// says "(refused)" under a green tick is worse than no badge at all.
        result: Option<Result<String, String>>,
    },
}

/// A `run_command` call the server paused for a decision. The turn is over until
/// `/coder/chat/approve` resumes it — nothing else arrives on the stream.
#[derive(Debug, Clone)]
pub struct Pending {
    pub call_id: String,
    /// What the user is being asked to allow. Empty when the model's call was
    /// malformed enough that no command could be read out of it — seen live,
    /// from a model that leaked `</tool_call>` as prose. The card must not offer
    /// to run something it cannot name, so the view checks this.
    pub command: String,
}

/// Which list the left sidebar is showing. One column, three lists, an icon
/// rail to switch them — rather than three stacked lists sharing 224px, which
/// is what made the sessions and the checkpoints fight over the same scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Sessions,
    Files,
    Checkpoints,
}

/// Which panel the bottom dock is showing. The dock only appears when at least
/// one of them has something in it, and a tab only appears with its content —
/// see [`State::dock_tabs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dock {
    #[default]
    Terminal,
    File,
    Diff,
}

#[derive(Debug, Default)]
pub struct State {
    /// The folder the agent works in. Every path it asks for is resolved
    /// against this and refused if it escapes.
    pub root: Option<PathBuf>,
    /// Server-side thread. Opened before the first turn streams, because
    /// answering a tool call needs the id the turn would otherwise carry.
    pub thread_id: Option<i64>,
    pub turns: Vec<Turn>,
    pub draft: String,
    pub sending: bool,
    /// Whether `run_command` is offered to the model at all. Off by default:
    /// reading and writing files is recoverable with the user's own git,
    /// running things is not.
    pub allow_commands: bool,
    /// Ask for a written plan before the loop starts. One extra tool-free call
    /// per turn, and hearth measures it as the single biggest quality lever for
    /// a local model — which is what this screen mostly runs. On by default for
    /// that reason; the header switch is for when the task is one step and the
    /// extra round trip is the slowest part of it.
    pub plan: bool,
    /// The agent's own commits over this folder, newest first — one per turn
    /// that changed a file. See [`crate::coder_git`]: they live in a git dir of
    /// ours, so none of this is in the user's history.
    pub checkpoints: Vec<crate::coder_git::Checkpoint>,
    /// The checkpoint whose diff is open, and the diff itself. `None` while it
    /// loads, so the panel can say so rather than appear empty.
    pub reviewing: Option<(String, Option<String>)>,
    /// A restore that has been asked for and not yet confirmed. Restoring
    /// throws away every change since that checkpoint, including the user's own
    /// edits — one click is not enough to authorise that.
    pub restore_armed: Option<String>,
    /// Why there are no checkpoints, when there should be. git may not be
    /// installed; the turn still ran, so this belongs next to the timeline
    /// rather than in the error banner that means "the turn failed".
    pub checkpoint_error: Option<String>,
    /// Whether the file pane is showing. Off by default — the transcript is
    /// what this screen is for, and the tree is for the moments it is not.
    pub files_open: bool,
    /// The tree as drawn: only what is expanded, re-walked rather than cached
    /// (see [`crate::coder_files`]). A turn writes files, so a cache here would
    /// need invalidating on every event that matters.
    pub tree: Vec<crate::coder_files::Entry>,
    pub expanded: std::collections::BTreeSet<PathBuf>,
    /// The file being read, and its text or why it cannot be shown. Read
    /// synchronously: it is one bounded `fs::read` off a click, and a Task for
    /// it would cost a message round trip to save nothing.
    pub viewing: Option<(PathBuf, Result<String, String>)>,
    /// The user's own shell, open in the workspace root.
    ///
    /// Not the agent's `run_command` — that one is the model's request and goes
    /// through the approval gate. This is the user's own typing, which needs no
    /// gate any more than their own terminal does. It exists because the loop
    /// after a turn is "run the tests it just changed", and routing that through
    /// the model costs two clicks, a round trip and tokens to type `cargo test`.
    ///
    /// `None` until asked for: a shell process per app launch, in a folder that
    /// may not be open yet, is a process nobody asked to start.
    pub term: Option<crate::coder_term::Session>,
    /// Ids handed to terminals so far. The widget's subscription is keyed on it,
    /// so reopening must not reuse one.
    term_seq: u64,
    pub pending: Option<Pending>,
    /// Tool rows whose output the user has expanded.
    pub open_tools: std::collections::HashSet<usize>,
    pub error: Option<String>,
    /// Providers the proxy knows, for the header dropdowns.
    pub catalog: Vec<ProviderEntry>,
    /// Provider/model override. Empty means the server's default — which is
    /// worth overriding: the resolved default here is `llama3`, and a model
    /// that cannot hold a multi-step tool loop makes this screen look broken
    /// rather than badly configured.
    pub provider: String,
    pub model: String,
    /// The prompt being sent, held across the thread-creation round trip.
    in_flight: String,
    /// Whether this turn has produced any assistant text yet. A turn that ends
    /// without one is the failure mode that reads as a hang, so it gets said
    /// out loud rather than rendering as nothing.
    answered: bool,
    /// Whether this turn had a command refused because the session has commands
    /// off. A silent turn after that is the refusal landing — blaming the model
    /// would send the user to change the wrong setting.
    refused_for_commands_off: bool,
    /// Past sessions, newest first. The server persists coder threads already,
    /// so this is a fetch rather than a store of our own — unlike the assistant's
    /// history, whose chat endpoint is stateless.
    pub threads: Vec<CoderThreadSummary>,
    /// A `load_threads` fetch is in flight — the sessions sidebar's only way to
    /// tell "nothing here yet" from "still asking", which look identical from
    /// an empty `threads`.
    pub threads_loading: bool,
    /// Same, for `load_checkpoints`.
    pub checkpoints_loading: bool,
    /// Seconds the turn in flight has been running. A local model can sit for
    /// minutes on one step, and a panel with no clock on it is indistinguishable
    /// from a hung one.
    pub elapsed: u32,
    /// A spinner frame, ticking while a turn, a tool, or a sidebar fetch is in
    /// motion — see [`Message::AnimTick`]. Only ever read `% frame_count`, so
    /// wrapping on overflow is fine.
    pub frame: u8,
    /// A decision has been sent and the resumed stream has not answered yet.
    ///
    /// `pending` is deliberately still set through this: the decision only
    /// counts once the server acts on it, and clearing it on the way out left
    /// the two sides disagreeing when the request failed in transport — the
    /// server still holding the call, the UI with no card left to retry from,
    /// and every later send refused with "thread has a command awaiting
    /// approval". Seen live.
    resuming: bool,
    /// Which list the left sidebar shows. [`Pane::Files`] and [`Self::files_open`]
    /// are the same fact — the bool is the one that persists to settings, the
    /// enum is the one the rail reads — so every write goes through
    /// [`Self::select_pane`].
    pub pane: Pane,
    /// Which bottom-dock tab is selected. Ignored when that tab has no content;
    /// the view falls back to the first tab that does.
    pub dock: Dock,
    /// Whether the preview pane is showing. The webview itself is not here —
    /// it is `!Send` and lives on the UI thread, see [`crate::coder_browser`].
    pub browser_open: bool,
    /// The URL bar's text, which is not the page's URL until it is submitted.
    pub browser_draft: String,
    /// The last URL actually handed to the preview, so reopening the pane
    /// returns to it rather than to a blank page.
    pub browser_url: String,
}

impl State {
    pub fn with_root(root: &str) -> Self {
        let root = root.trim();
        Self {
            root: (!root.is_empty()).then(|| PathBuf::from(root)),
            ..Self::default()
        }
    }

    /// The screen as the settings file left it.
    pub fn restored(root: &str, provider: String, model: String, plan: bool) -> Self {
        Self { provider, model, plan, ..Self::with_root(root) }
    }

    /// What the turn is waiting on, most specific first: the user, then the tool
    /// in flight, then the model. Ported from hearth's composer, and the
    /// ordering is the point — a counter climbing past two minutes next to a
    /// bare "thinking" is how a wait on the approval gate, or on a test suite,
    /// gets read as a hang.
    pub fn activity(&self) -> &str {
        // `pending` survives into the resume, so the decision being *sent* is
        // no longer a wait on the user.
        if !self.sending && self.pending.is_some() {
            return "waiting for you";
        }
        match self.turns.iter().rev().find(|t| matches!(t, Turn::Tool { result: None, .. })) {
            Some(Turn::Tool { label, .. }) => label,
            _ => "thinking",
        }
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.catalog.iter().map(|p| p.id.clone()).collect()
    }

    /// Models the chosen provider offers; every provider's when none is picked,
    /// since the proxy resolves an alias to its provider on its own.
    pub fn model_options(&self) -> Vec<String> {
        self.catalog
            .iter()
            .filter(|p| self.provider.is_empty() || p.id == self.provider)
            .flat_map(|p| p.models.options.iter().cloned())
            .collect()
    }

    pub fn root_label(&self) -> String {
        match &self.root {
            Some(p) => p.display().to_string(),
            None => "No folder open".to_string(),
        }
    }

    /// The last thing the agent actually said, for a completion toast.
    pub fn last_reply(&self) -> &str {
        self.turns
            .iter()
            .rev()
            .find_map(|t| match t {
                Turn::Assistant { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn push_assistant(&mut self, text: String) {
        self.turns.push(Turn::Assistant { md: markdown::parse(&text).collect(), text });
    }

    /// Attach an outcome to the tool row still waiting for one. The server runs
    /// a round's calls in order and echoes each result before issuing the next,
    /// so the oldest unanswered row is the right one — provided no earlier turn
    /// left one behind, which is what [`Self::close_open_tools`] guarantees.
    fn resolve_tool(&mut self, result: Result<String, String>) {
        if let Some(Turn::Tool { result: slot, .. }) =
            self.turns.iter_mut().find(|t| matches!(t, Turn::Tool { result: None, .. }))
        {
            *slot = Some(result);
        }
    }

    /// Open a shell in the workspace root, and put the keyboard in it.
    ///
    /// A failure here is the *terminal* failing, not the turn: it lands in the
    /// banner because the user just asked for this and nothing else explains an
    /// empty drawer.
    fn open_terminal(&mut self) -> Task<Message> {
        let Some(root) = self.root.clone() else { return Task::none() };
        self.term_seq += 1;
        match crate::coder_term::open(self.term_seq, &root) {
            Ok(session) => {
                let id = session.0.widget_id().clone();
                self.term = Some(session);
                iced_term::TerminalView::focus(id)
            }
            Err(e) => {
                self.error = Some(e);
                Task::none()
            }
        }
    }

    /// Re-walk the visible part of the tree. Cheap: it reads the root plus the
    /// directories the user opened, and nothing else.
    fn refresh_tree(&mut self) {
        self.tree = match (&self.root, self.files_open) {
            (Some(root), true) => crate::coder_files::flatten(root, &self.expanded),
            _ => Vec::new(),
        };
    }

    /// Switch the left sidebar, keeping [`Self::files_open`] in step. The tree
    /// is walked when it is on screen and dropped when it is not, so the pane
    /// switch is also what starts and stops the walking.
    fn select_pane(&mut self, pane: Pane) {
        self.pane = pane;
        self.files_open = pane == Pane::Files;
        self.refresh_tree();
    }

    /// The dock tabs that currently have something behind them, in a fixed
    /// order. Empty means no dock: an empty tab strip over an empty panel is
    /// worse than the space it takes.
    pub fn dock_tabs(&self) -> Vec<Dock> {
        let mut tabs = Vec::new();
        if self.term.is_some() {
            tabs.push(Dock::Terminal);
        }
        if self.viewing.is_some() {
            tabs.push(Dock::File);
        }
        if self.reviewing.is_some() {
            tabs.push(Dock::Diff);
        }
        tabs
    }

    /// The tab actually to draw: the selected one when it still has content,
    /// otherwise the first that does. Closing the open file while its tab is
    /// selected must land on the terminal, not on a blank dock.
    pub fn dock_shown(&self) -> Option<Dock> {
        let tabs = self.dock_tabs();
        tabs.iter().find(|t| **t == self.dock).or_else(|| tabs.first()).copied()
    }

    /// Close any row the ending turn never answered.
    ///
    /// A turn that fails mid-round leaves a row open forever, and the next
    /// turn's first result would then fill *that* row instead of its own —
    /// every later row off by one, each labelled with someone else's output.
    fn close_open_tools(&mut self) {
        for t in &mut self.turns {
            if let Turn::Tool { result: result @ None, .. } = t {
                *result = Some(Err("the turn ended before this call was answered".into()));
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    PickRoot,
    RootPicked(Option<String>),
    DraftChanged(String),
    Send,
    /// The session thread, opened before the first turn.
    ThreadOpened(Result<i64, String>),
    Event(CoderEvent),
    /// A delegated call finished on this machine; the server is still waiting.
    ToolRan { call_id: String, result: String },
    ToolPosted(Result<(), String>),
    ToggleTool(usize),
    /// Approve or refuse the paused `run_command`.
    Decide(bool),
    ToggleCommands(bool),
    TogglePlan(bool),
    /// The checkpoint repo is ready (or could not be made) — the turn follows.
    Baselined(Result<(), String>),
    /// The turn's own commit landed; the payload says whether it made a row.
    Committed(Result<bool, String>),
    CheckpointsLoaded(Result<Vec<crate::coder_git::Checkpoint>, String>),
    ReviewCheckpoint(String),
    DiffLoaded(Result<String, String>),
    CloseReview,
    /// First press arms, second restores — see [`State::restore_armed`].
    RestoreCheckpoint(String),
    Restored(Result<(), String>),
    ToggleFiles,
    /// The left rail: pick which list the sidebar shows.
    SelectPane(Pane),
    /// The bottom dock's tab strip.
    SelectDock(Dock),
    RefreshTree,
    ToggleDir(PathBuf),
    OpenFile(PathBuf),
    CloseFile,
    /// Open the shell drawer, or close it and end the shell.
    ToggleTerminal,
    /// Everything the terminal widget produces — keystrokes, resizes, PTY output.
    Term(iced_term::Event),
    /// Type a command into the open shell (opening it first if needed). The
    /// affordance on a `run_command` row: re-run what the agent ran, by hand,
    /// where you can answer its prompts.
    SendToTerminal(String),
    ProviderChanged(String),
    ModelChanged(String),
    CatalogLoaded(Result<Vec<ProviderEntry>, String>),
    /// Past sessions, refetched rather than patched — the server owns titles and
    /// ordering, and a turn changes both.
    ThreadsLoaded(Result<Vec<CoderThreadSummary>, String>),
    OpenThread(i64),
    ThreadLoaded(Result<Box<agent_platform_client::types::CoderThreadOut>, String>),
    DeleteThread(i64),
    /// One second of a turn in flight.
    Tick,
    /// One frame of the spinner shown while a turn, a tool, or a sidebar fetch
    /// is in flight — see [`State::frame`].
    AnimTick,
    New,
    LinkClicked(String),
    DismissError,

    // --- preview pane (see `crate::coder_browser`) ---------------------------
    ToggleBrowser,
    BrowserUrlChanged(String),
    /// Enter in the URL bar, or the reload button when the draft has moved on.
    BrowserGo,
    BrowserBack,
    BrowserForward,
    BrowserReload,
    /// The window moved or resized, or the screen changed — the child window
    /// has to be told where the strip is now.
    BrowserSync,
    /// Take the preview off the screen without forgetting the page: leaving the
    /// Coder screen, or a modal going up over it.
    BrowserHide,
    /// A preview command came back. Only failures are kept — a working preview
    /// says so by being visible.
    BrowserDone(Result<(), String>),
}

/// Fetch the list of past sessions for the history sidebar.
pub fn load_threads(state: &mut State, client: &Client) -> Task<Message> {
    state.threads_loading = true;
    let client = client.clone();
    Task::perform(
        async move { client.coder_threads().await.map(|r| r.threads).map_err(|e| e.to_string()) },
        Message::ThreadsLoaded,
    )
}

/// Read the checkpoint timeline for the open folder.
///
/// Takes the whole state rather than a root because "no folder open" is the
/// common case at startup, and every caller would otherwise repeat the check.
pub fn load_checkpoints(state: &mut State) -> Task<Message> {
    let Some(root) = state.root.clone() else { return Task::none() };
    state.checkpoints_loading = true;
    Task::perform(async move { crate::coder_git::list(&root).await }, Message::CheckpointsLoaded)
}

/// Fetch the provider catalog for the header dropdowns.
pub fn load_catalog(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { client.llm_providers().await.map(|c| c.providers).map_err(|e| e.to_string()) },
        Message::CatalogLoaded,
    )
}

/// Human-readable summary of a call, for the transcript row. `run_command`
/// shows the command itself — a row reading "run_command" is one the user
/// learns to approve without reading.
/// Rebuild a transcript from a thread's stored history.
///
/// The server keeps the OpenAI-shaped log — user turns, assistant turns that
/// may carry `tool_calls`, and `tool` messages answering them — so reopening a
/// session means turning that back into the rows the live stream produces. The
/// two have to agree: a reopened thread that renders differently from the one
/// you just had open is a thread you cannot trust to be the same thread.
///
/// Tool results are matched to calls in order, exactly as [`State::resolve_tool`]
/// does live, rather than by `tool_call_id` — the ids are the model's and a
/// model that reuses or mangles one would drop a row on the floor.
pub fn rebuild_turns(messages: &[serde_json::Value]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or_default();
        let content = m.get("content").and_then(|c| c.as_str()).unwrap_or_default();
        match role {
            "user" => turns.push(Turn::User(content.to_string())),
            "tool" => {
                let result = if content.starts_with("Error:") {
                    Err(content.to_string())
                } else {
                    Ok(content.to_string())
                };
                if let Some(Turn::Tool { result: slot, .. }) =
                    turns.iter_mut().find(|t| matches!(t, Turn::Tool { result: None, .. }))
                {
                    *slot = Some(result);
                }
            }
            "assistant" => {
                // The step that only called a tool has empty content; rendering
                // it would put a blank bubble above every tool row.
                if !content.trim().is_empty() {
                    turns.push(Turn::Assistant {
                        md: markdown::parse(content).collect(),
                        text: content.to_string(),
                    });
                }
                for call in m.get("tool_calls").and_then(|c| c.as_array()).into_iter().flatten() {
                    let f = call.get("function").unwrap_or(call);
                    let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                    // Arguments ride as a JSON *string* in the OpenAI shape, so
                    // they need parsing before `label_for` can read a path out.
                    let args = match f.get("arguments") {
                        Some(serde_json::Value::String(s)) => {
                            serde_json::from_str(s).unwrap_or(serde_json::Value::Null)
                        }
                        Some(v) => v.clone(),
                        None => serde_json::Value::Null,
                    };
                    turns.push(Turn::Tool { label: label_for(name, &args), result: None });
                }
            }
            _ => {} // system prompts are not part of the conversation
        }
    }
    // A session that was interrupted mid-call is being *read*, not resumed —
    // leaving a row spinning forever would say otherwise.
    for t in &mut turns {
        if let Turn::Tool { result: result @ None, .. } = t {
            *result = Some(Err("never answered — the session ended here".into()));
        }
    }
    turns
}

/// The command a `run_command` call asks to run, or empty when none can be read
/// out of it. A model that leaks its tool syntax as prose gets its call salvaged
/// server-side with whatever arguments survived, which can be nothing.
fn command_of(args: &serde_json::Value) -> String {
    args.get("command").and_then(|c| c.as_str()).unwrap_or_default().trim().to_string()
}

fn label_for(name: &str, args: &serde_json::Value) -> String {
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "run_command" => format!("$ {}", arg("command")),
        "write_file" => format!("write_file {}", arg("path")),
        "read_file" => format!("read_file {}", arg("path")),
        // The query, not the scope: `search "fn resolve_in_root"` is the row a
        // user can read past, `search src` is one they have to expand.
        "search" => format!("search {:?}", arg("query")),
        // It takes no arguments, so the default arm would render `repo_map({})`.
        "repo_map" => "repo_map".to_string(),
        "list_dir" => {
            let p = arg("path");
            format!("list_dir {}", if p.is_empty() { "." } else { p })
        }
        other => format!("{other}({args})"),
    }
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    // Any frame off the resumed stream is the server acting on the decision, so
    // the call it answered is no longer pending. A `Failed` is the opposite —
    // the decision never landed, and its arm puts the card back.
    if state.resuming
        && matches!(message, Message::Event(ref e) if !matches!(e, CoderEvent::Failed(_)))
    {
        state.resuming = false;
        state.pending = None;
    }
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::PickRoot => Task::future(async {
            rfd::AsyncFileDialog::new()
                .set_title("Open a folder to code in")
                .pick_folder()
                .await
                .map(|h| h.path().display().to_string())
        })
        .map(Message::RootPicked),
        Message::RootPicked(None) => Task::none(),
        Message::RootPicked(Some(path)) => {
            // A new folder is a new session: the thread carries its workspace
            // root server-side, so reusing it would point the agent at the old
            // one while the screen showed the new. The settings around the
            // conversation are not part of the conversation and survive.
            *state = State {
                allow_commands: state.allow_commands,
                plan: state.plan,
                catalog: std::mem::take(&mut state.catalog),
                provider: std::mem::take(&mut state.provider),
                model: std::mem::take(&mut state.model),
                threads: std::mem::take(&mut state.threads),
                // The pane's visibility is a preference; what is *in* it
                // belongs to the folder that just went away.
                files_open: state.files_open,
                pane: state.pane,
                dock: state.dock,
                // The preview is aimed at a dev server, which the new folder
                // may well also be serving — and the child window is alive
                // either way, so forgetting the URL here would only strand it.
                browser_open: state.browser_open,
                browser_url: std::mem::take(&mut state.browser_url),
                browser_draft: std::mem::take(&mut state.browser_draft),
                ..State::with_root(&path)
            };
            state.refresh_tree();
            // The new folder may have been coded in before — its checkpoints
            // are in the folder, not in this app's state.
            load_checkpoints(state)
        }
        Message::DraftChanged(v) => {
            state.draft = v;
            Task::none()
        }
        Message::ToggleCommands(on) => {
            state.allow_commands = on;
            Task::none()
        }
        Message::TogglePlan(on) => {
            state.plan = on;
            Task::none()
        }

        // --- checkpoints ---------------------------------------------------
        // None of these can fail a turn. git may not be installed, and the
        // agent's work is the point — the history of it is a convenience, so it
        // reports itself in the timeline and nowhere else.
        Message::Baselined(r) => {
            state.checkpoint_error = r.err();
            Task::none()
        }
        // A turn that changed nothing on disk gets no row.
        Message::Committed(Ok(false)) => Task::none(),
        Message::Committed(Ok(true)) => load_checkpoints(state),
        Message::Committed(Err(e)) => {
            state.checkpoint_error = Some(e);
            Task::none()
        }
        Message::CheckpointsLoaded(Ok(list)) => {
            state.checkpoints = list;
            state.checkpoint_error = None;
            state.checkpoints_loading = false;
            Task::none()
        }
        Message::CheckpointsLoaded(Err(e)) => {
            state.checkpoint_error = Some(e);
            state.checkpoints_loading = false;
            Task::none()
        }
        Message::ReviewCheckpoint(sha) => {
            // Opened empty and filled when the diff arrives — a panel that
            // appears blank for a second reads as a checkpoint with no changes.
            state.reviewing = Some((sha.clone(), None));
            state.restore_armed = None;
            state.dock = Dock::Diff;
            let Some(root) = state.root.clone() else { return Task::none() };
            Task::perform(
                async move { crate::coder_git::diff(&root, &sha).await },
                Message::DiffLoaded,
            )
        }
        Message::DiffLoaded(Ok(text)) => {
            if let Some((_, slot)) = state.reviewing.as_mut() {
                *slot = Some(text);
            }
            Task::none()
        }
        Message::DiffLoaded(Err(e)) => {
            state.reviewing = None;
            state.error = Some(format!("Could not read that checkpoint: {e}"));
            Task::none()
        }
        Message::CloseReview => {
            state.reviewing = None;
            state.restore_armed = None;
            Task::none()
        }
        Message::RestoreCheckpoint(sha) => {
            // First press arms, second acts. Restoring is `git reset --hard`:
            // it takes the files back, and everything since — the agent's later
            // turns *and* whatever the user typed in their own editor — goes
            // with them. One click cannot be enough for that.
            if state.restore_armed.as_deref() != Some(sha.as_str()) {
                state.restore_armed = Some(sha);
                return Task::none();
            }
            state.restore_armed = None;
            let Some(root) = state.root.clone() else { return Task::none() };
            Task::perform(
                async move { crate::coder_git::restore(&root, &sha).await },
                Message::Restored,
            )
        }
        // The later checkpoints are gone from the branch now, so the timeline is
        // refetched rather than trimmed locally.
        Message::Restored(Ok(())) => {
            state.reviewing = None;
            load_checkpoints(state)
        }
        Message::Restored(Err(e)) => {
            state.error = Some(format!("Could not restore that checkpoint: {e}"));
            Task::none()
        }

        // --- files -----------------------------------------------------------
        Message::ToggleFiles => {
            // Re-walked on open rather than kept warm: a turn writes files, and
            // the pane is only ever wrong for as long as it is hidden.
            state.select_pane(if state.files_open { Pane::Sessions } else { Pane::Files });
            Task::none()
        }
        Message::SelectPane(pane) => {
            state.select_pane(pane);
            Task::none()
        }
        Message::SelectDock(dock) => {
            state.dock = dock;
            Task::none()
        }
        Message::RefreshTree => {
            state.refresh_tree();
            Task::none()
        }
        Message::ToggleDir(path) => {
            if !state.expanded.remove(&path) {
                state.expanded.insert(path);
            }
            state.refresh_tree();
            Task::none()
        }
        Message::OpenFile(path) => {
            let text = crate::coder_files::read_capped(&path);
            state.viewing = Some((path, text));
            state.dock = Dock::File;
            Task::none()
        }
        Message::CloseFile => {
            state.viewing = None;
            Task::none()
        }
        // --- terminal --------------------------------------------------------
        Message::ToggleTerminal => {
            // Closing drops the `Session`, which ends the shell and its PTY.
            // Deliberate: a hidden shell holding a dev server, with no window
            // to see it in, is a process the user cannot reason about.
            if state.term.take().is_some() {
                return Task::none();
            }
            state.dock = Dock::Terminal;
            state.open_terminal()
        }
        Message::Term(iced_term::Event::BackendCall(_, cmd)) => {
            let Some(session) = state.term.as_mut() else { return Task::none() };
            // `Shutdown` is the shell exiting — `exit`, or Ctrl-D. The drawer
            // goes with it rather than showing a dead grid.
            if session.0.handle(iced_term::Command::ProxyToBackend(cmd))
                == iced_term::actions::Action::Shutdown
            {
                state.term = None;
                // Whatever it did to the folder happened while it was open.
                state.refresh_tree();
            }
            Task::none()
        }
        Message::SendToTerminal(command) => {
            let open = if state.term.is_some() { Task::none() } else { state.open_terminal() };
            let Some(session) = state.term.as_mut() else { return open };
            crate::coder_term::send_line(session, &command);
            open
        }
        Message::ProviderChanged(v) => {
            // The picked model belongs to the old provider; keep it only if the
            // new one also offers it.
            state.provider = v;
            if !state.model_options().contains(&state.model) {
                state.model.clear();
            }
            Task::none()
        }
        Message::ModelChanged(v) => {
            state.model = v;
            Task::none()
        }
        // Only configured providers can answer, so only those are offered; the
        // rest stay in Settings → Providers until they have a key or endpoint.
        Message::CatalogLoaded(Ok(providers)) => {
            state.catalog = providers.into_iter().filter(|p| p.configured).collect();
            Task::none()
        }
        // The dropdowns stay empty and the turn runs on server defaults.
        Message::CatalogLoaded(Err(_)) => Task::none(),

        Message::ThreadsLoaded(Ok(threads)) => {
            state.threads = threads;
            state.threads_loading = false;
            Task::none()
        }
        // The sidebar just stays as it was; the live session is unaffected.
        Message::ThreadsLoaded(Err(_)) => {
            state.threads_loading = false;
            Task::none()
        }
        Message::OpenThread(id) => {
            if state.sending {
                return Task::none();
            }
            let c = client.clone();
            Task::perform(
                async move {
                    c.coder_thread(id).await.map(Box::new).map_err(|e| e.to_string())
                },
                Message::ThreadLoaded,
            )
        }
        Message::ThreadLoaded(Ok(thread)) => {
            // The root travels with the thread. Reopening a session against
            // whatever folder happened to be on screen would point the agent at
            // one project while the transcript describes another.
            if let Some(root) = thread.workspace_root.as_deref().filter(|r| !r.trim().is_empty()) {
                state.root = Some(PathBuf::from(root));
            }
            state.thread_id = Some(thread.thread_id);
            state.turns = rebuild_turns(&thread.messages);
            state.open_tools.clear();
            state.pending = None;
            state.error = None;
            state.reviewing = None;
            state.restore_armed = None;
            // The session may have come from another folder, and both the
            // timeline and the tree belong to the folder.
            state.expanded.clear();
            state.viewing = None;
            state.refresh_tree();
            Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                load_checkpoints(state),
            ])
        }
        Message::ThreadLoaded(Err(e)) => {
            state.error = Some(format!("Could not open that session: {e}"));
            Task::none()
        }
        Message::DeleteThread(id) => {
            state.threads.retain(|t| t.id != id);
            // Deleting the open session leaves the transcript on screen but
            // detaches it: the next send opens a new thread rather than writing
            // into one the server no longer has.
            if state.thread_id == Some(id) {
                state.thread_id = None;
            }
            let c = client.clone();
            Task::perform(
                async move { c.delete_coder_thread(id).await.map_err(|e| e.to_string()) },
                |_| Message::Tick,
            )
        }
        // A turn parked on the approval gate is still a turn the user is
        // waiting through, and it is the *longest* wait available here — the
        // clock stopping on it is the one case hearth's counter exists for.
        Message::Tick => {
            if state.sending || state.pending.is_some() {
                state.elapsed += 1;
            }
            Task::none()
        }
        Message::AnimTick => {
            state.frame = state.frame.wrapping_add(1);
            Task::none()
        }
        Message::Send => {
            let prompt = state.draft.trim().to_string();
            let Some(root) = state.root.clone() else { return Task::none() };
            if prompt.is_empty() || state.sending || state.pending.is_some() {
                return Task::none();
            }
            state.turns.push(Turn::User(prompt.clone()));
            state.draft.clear();
            state.sending = true;
            state.error = None;
            state.answered = false;
            state.refused_for_commands_off = false;
            state.elapsed = 0;
            state.in_flight = prompt;

            let turn = match state.thread_id {
                Some(_) => send_turn(state, client),
                None => {
                    let c = client.clone();
                    let path = root.display().to_string();
                    Task::perform(
                        async move {
                            c.create_coder_thread(&path)
                                .await
                                .map(|t| t.thread_id)
                                .map_err(|e| e.to_string())
                        },
                        Message::ThreadOpened,
                    )
                }
            };
            // Chained, not batched: the baseline has to be taken *before* the
            // first tool writes anything, or that turn's first checkpoint would
            // contain its own changes and show as having changed nothing. It is
            // one `rev-parse` once the repo exists.
            Task::perform(async move { crate::coder_git::ensure_repo(&root).await }, Message::Baselined)
                .chain(turn)
        }
        Message::ThreadOpened(Ok(id)) => {
            state.thread_id = Some(id);
            send_turn(state, client)
        }
        Message::ThreadOpened(Err(e)) => {
            state.sending = false;
            state.error = Some(e);
            Task::none()
        }

        // An empty one is normal — the step that only called a tool sends one —
        // so it is dropped rather than rendered as a blank bubble. `answered`
        // is what stops a whole turn of them from being silent.
        Message::Event(CoderEvent::Assistant(text)) => {
            if text.trim().is_empty() {
                return Task::none();
            }
            state.answered = true;
            state.push_assistant(text);
            iced::widget::operation::snap_to_end(transcript_id())
        }
        // The plan the turn opened with. Rendered as an ordinary assistant row
        // because that is what a reopened session rebuilds it as — the server
        // persists it as a plain assistant message, and the two views of one
        // session have to agree. It does *not* count as the turn's answer: a
        // model that plans and then dies silently is still a silent turn, and
        // that is the failure this screen has to name out loud.
        Message::Event(CoderEvent::Plan(text)) => {
            if text.trim().is_empty() {
                return Task::none();
            }
            state.push_assistant(text);
            iced::widget::operation::snap_to_end(transcript_id())
        }
        Message::Event(CoderEvent::ToolCall { call_id, name, arguments }) => {
            state.turns.push(Turn::Tool { label: label_for(&name, &arguments), result: None });
            // The server is blocked from here until the result is posted, so
            // every branch below must produce one — including "no root", which
            // cannot happen from the UI but would hang the turn if it did.
            let Some(root) = state.root.clone() else {
                return Task::done(Message::ToolRan {
                    call_id,
                    result: "Error: no workspace folder is open on the desktop.".into(),
                });
            };
            let allow = state.allow_commands;
            Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                Task::perform(
                    async move {
                        let result =
                            crate::coder_tools::execute(&root, &name, &arguments, allow).await;
                        (call_id, result)
                    },
                    |(call_id, result)| Message::ToolRan { call_id, result },
                ),
            ])
        }
        Message::ToolRan { call_id, result } => {
            let Some(thread) = state.thread_id else { return Task::none() };
            let c = client.clone();
            Task::perform(
                async move {
                    c.coder_tool_result(thread, &call_id, &result).await.map_err(|e| e.to_string())
                },
                Message::ToolPosted,
            )
        }
        // The turn is parked on a result that never arrives, and the stream
        // gives no sign of it — so say so rather than leave a spinner running
        // until the server's 300s timeout.
        Message::ToolPosted(Err(e)) => {
            state.sending = false;
            state.close_open_tools();
            state.error = Some(format!("Could not hand the tool result back: {e}"));
            Task::none()
        }
        Message::ToolPosted(Ok(())) => Task::none(),
        // The executor reports its own failures as text the model can act on,
        // so an `Error:` prefix is the only signal a call went wrong.
        Message::Event(CoderEvent::ToolResult { content, .. }) => {
            let result =
                if content.starts_with("Error:") { Err(content) } else { Ok(content) };
            state.resolve_tool(result);
            iced::widget::operation::snap_to_end(transcript_id())
        }
        // No row here. The resumed turn emits a real `tool_call` for the very
        // same call, and a row pushed now would sit beside it as a duplicate —
        // seen on screen before it was seen in the code. The pending card is
        // what shows the command while the decision is outstanding.
        Message::Event(CoderEvent::ApprovalRequired { call_id, name, arguments }) => {
            // The server offers `run_command` whatever this session allows, so
            // with commands off the user was being asked to approve something
            // the executor would then refuse anyway — two gates, one answer.
            // Refuse it here and let the model read why.
            if !state.allow_commands {
                state.turns.push(Turn::Tool {
                    label: label_for(&name, &arguments),
                    result: Some(Err("commands are off for this session".into())),
                });
                let task = resume_after_decision(state, client, &call_id, false);
                state.refused_for_commands_off = true;
                return task;
            }
            state.pending = Some(Pending { call_id, command: command_of(&arguments) });
            state.sending = false;
            iced::widget::operation::snap_to_end(transcript_id())
        }
        Message::Event(CoderEvent::Failed(e)) => {
            state.sending = false;
            state.close_open_tools();
            // A decision that never reached the server leaves the server still
            // holding the call: every later send comes back "thread has a
            // command awaiting approval". The card goes back up so the same
            // decision can be sent again, which is the only way out from here.
            state.error = Some(if state.resuming {
                state.resuming = false;
                format!("The decision did not reach the server ({e}). Answer it again.")
            } else {
                e
            });
            Task::none()
        }
        // `Done` also arrives right behind an approval pause — the route
        // returns rather than staying open — so the row waiting on that
        // decision is not one the turn failed to answer.
        Message::Event(CoderEvent::Done) => {
            state.sending = false;
            if state.pending.is_none() {
                state.close_open_tools();
                // A turn that ends having said nothing renders as nothing, which
                // is indistinguishable from a hang — and it is the *normal* way
                // a model too weak for a tool loop fails, not an edge case. The
                // server's resolved default is `llama3`, so this is the first
                // thing a new user hits.
                if !state.answered {
                    state.error = Some(if state.refused_for_commands_off {
                        "The command was not run — commands are off for this session. \
                         Turn them on in the header if the agent should be able to run \
                         things."
                            .into()
                    } else {
                        "The model ended the turn without replying. That usually means the \
                         model cannot hold a tool loop — pick a coding model in the header \
                         and try again."
                            .to_string()
                    });
                }
            }
            // The turn is the one thing that changes these files behind the
            // user's back, so the pane is re-walked when it ends.
            state.refresh_tree();
            // A turn parked on the approval gate is not over — its command has
            // not run yet, so committing here would checkpoint half of it.
            let checkpoint = match (state.pending.is_none(), state.root.clone()) {
                (true, Some(root)) => {
                    let message = state.in_flight.clone();
                    Task::perform(
                        async move { crate::coder_git::commit_all(&root, &message).await },
                        Message::Committed,
                    )
                }
                _ => Task::none(),
            };
            // The server names a thread from its first turn and reorders the
            // list by recency, so the sidebar is refetched when a turn lands
            // rather than guessed at locally.
            return Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                load_threads(state, client),
                checkpoint,
            ]);
        }

        Message::Decide(approve) => {
            // Cloned, not taken: the call stays pending until the server acts on
            // the decision — see [`State::resuming`].
            let (Some(pending), Some(thread)) = (state.pending.clone(), state.thread_id) else {
                return Task::none();
            };
            if !approve {
                // No row exists for a refused call — none was ever pushed and
                // the resumed turn will not emit one — so this adds it.
                state.turns.push(Turn::Tool {
                    // An unreadable call has no command to name, and a bare `$`
                    // is a row that says nothing.
                    label: if pending.command.is_empty() {
                        "run_command (unreadable)".to_string()
                    } else {
                        format!("$ {}", pending.command)
                    },
                    result: Some(Err("refused by the user".into())),
                });
            }
            let _ = thread;
            resume_after_decision(state, client, &pending.call_id, approve)
        }

        Message::ToggleTool(idx) => {
            if !state.open_tools.remove(&idx) {
                state.open_tools.insert(idx);
            }
            Task::none()
        }
        Message::New => {
            // The root survives; everything about the conversation does not.
            *state = State {
                root: state.root.clone(),
                allow_commands: state.allow_commands,
                plan: state.plan,
                catalog: std::mem::take(&mut state.catalog),
                provider: std::mem::take(&mut state.provider),
                model: std::mem::take(&mut state.model),
                threads: std::mem::take(&mut state.threads),
                // A new *conversation*, not a new folder: the file history and
                // the tree are the folder's, and outlive any session in it.
                checkpoints: std::mem::take(&mut state.checkpoints),
                files_open: state.files_open,
                pane: state.pane,
                dock: state.dock,
                browser_open: state.browser_open,
                browser_url: std::mem::take(&mut state.browser_url),
                browser_draft: std::mem::take(&mut state.browser_draft),
                tree: std::mem::take(&mut state.tree),
                expanded: std::mem::take(&mut state.expanded),
                ..State::default()
            };
            Task::none()
        }
        Message::LinkClicked(url) => {
            if url.starts_with("http://") || url.starts_with("https://") {
                crate::shell::reveal_path(&url);
            }
            Task::none()
        }
        Message::DismissError => {
            state.error = None;
            Task::none()
        }

        // --- preview pane ----------------------------------------------------
        Message::ToggleBrowser => {
            state.browser_open = !state.browser_open;
            if !state.browser_open {
                return crate::coder_browser::run(
                    crate::coder_browser::Cmd::Hide,
                    Message::BrowserDone,
                );
            }
            // Reopening returns to the page it was on; a first open with a URL
            // already typed goes straight there rather than to a blank pane.
            let url = if state.browser_url.is_empty() {
                normalize_url(&state.browser_draft)
            } else {
                Some(state.browser_url.clone())
            };
            match url {
                Some(url) => {
                    state.browser_url = url.clone();
                    state.browser_draft = url.clone();
                    crate::coder_browser::run(
                        crate::coder_browser::Cmd::Load(url),
                        Message::BrowserDone,
                    )
                }
                None => crate::coder_browser::run(
                    crate::coder_browser::Cmd::Show,
                    Message::BrowserDone,
                ),
            }
        }
        Message::BrowserUrlChanged(v) => {
            state.browser_draft = v;
            Task::none()
        }
        Message::BrowserGo => {
            let Some(url) = normalize_url(&state.browser_draft) else { return Task::none() };
            state.browser_url = url.clone();
            state.browser_draft = url.clone();
            state.browser_open = true;
            crate::coder_browser::run(crate::coder_browser::Cmd::Load(url), Message::BrowserDone)
        }
        Message::BrowserBack => {
            crate::coder_browser::run(crate::coder_browser::Cmd::Back, Message::BrowserDone)
        }
        Message::BrowserForward => {
            crate::coder_browser::run(crate::coder_browser::Cmd::Forward, Message::BrowserDone)
        }
        Message::BrowserReload => {
            crate::coder_browser::run(crate::coder_browser::Cmd::Reload, Message::BrowserDone)
        }
        Message::BrowserSync => {
            if !state.browser_open {
                return Task::none();
            }
            crate::coder_browser::run(crate::coder_browser::Cmd::Show, Message::BrowserDone)
        }
        Message::BrowserHide => {
            // The pane stays "open" as far as the screen is concerned — this is
            // the child window being taken off a screen it would otherwise
            // float over, not the user closing anything.
            crate::coder_browser::run(crate::coder_browser::Cmd::Hide, Message::BrowserDone)
        }
        Message::BrowserDone(Ok(())) => Task::none(),
        Message::BrowserDone(Err(e)) => {
            state.browser_open = false;
            state.error = Some(e);
            Task::none()
        }
    }
}

/// What to hand the preview for what the user typed. `None` for an empty box —
/// there is nothing to navigate to, and a blank submit should not blank the page.
///
/// A bare `localhost:5173` or `127.0.0.1:8080` is the whole point of this pane,
/// and neither is a URL until it has a scheme, so one is added. Anything already
/// carrying a scheme is passed through untouched.
fn normalize_url(draft: &str) -> Option<String> {
    let draft = draft.trim();
    if draft.is_empty() {
        return None;
    }
    if draft.contains("://") || draft.starts_with("about:") {
        return Some(draft.to_string());
    }
    Some(format!("http://{draft}"))
}

/// Answer a paused `run_command` and pick the turn back up. Two callers: the
/// user's decision, and the automatic refusal when commands are off for the
/// session — both have to resume the stream, or the turn is simply abandoned.
fn resume_after_decision(
    state: &mut State,
    client: &Client,
    call_id: &str,
    approve: bool,
) -> Task<Message> {
    let Some(thread) = state.thread_id else { return Task::none() };
    // `pending` stays until the server acts on this — see [`State::resuming`].
    state.resuming = true;
    state.sending = true;
    state.answered = false;
    state.elapsed = 0;
    let some_if_set = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    let body = serde_json::json!({
        "thread_id": thread,
        "call_id": call_id,
        "approve": approve,
        "delegate_tools": true,
        // The resumed turn rebuilds the system prompt from scratch, so the notes
        // have to be sent again — a turn that knew the workspace before the
        // approval gate and not after is one the gate silently lobotomised.
        "mode_instruction": workspace_notes(state),
        "provider": some_if_set(&state.provider),
        "model": some_if_set(&state.model),
    });
    Task::run(coder_stream(client.clone(), "/api/v1/coder/chat/approve", body), Message::Event)
}

/// What the agent already knows about this workspace, for the system prompt.
/// `None` with no folder open, which is also the only case where there is
/// nothing it could be about.
///
/// Read off disk per turn rather than cached: the agent rewrites the file
/// mid-session and the user may edit it under us, and re-reading four kilobytes
/// is cheaper than either of those going unnoticed.
fn workspace_notes(state: &State) -> Option<String> {
    state.root.as_deref().map(crate::coder_notes::block)
}

/// Start the streamed turn for `state.in_flight`.
///
/// `delegate_tools` is what puts the filesystem on this machine, and
/// `auto_approve_commands` stays false so every command is read by a human
/// first — this screen has no checkpoint to undo one with.
fn send_turn(state: &State, client: &Client) -> Task<Message> {
    let some_if_set = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    let body = serde_json::json!({
        "message": state.in_flight,
        "thread_id": state.thread_id,
        "workspace_root": state.root.as_ref().map(|p| p.display().to_string()),
        "allow_commands": state.allow_commands,
        "auto_approve_commands": false,
        "delegate_tools": true,
        "plan": state.plan,
        // The server merges this into the system prompt rather than storing it
        // as a message, so the notes never accumulate in the thread history —
        // one copy per turn, always the current file.
        "mode_instruction": workspace_notes(state),
        "provider": some_if_set(&state.provider),
        "model": some_if_set(&state.model),
    });
    Task::batch([
        iced::widget::operation::snap_to_end(transcript_id()),
        Task::run(coder_stream(client.clone(), "/api/v1/coder/chat/stream", body), Message::Event),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn open_state() -> State {
        State { thread_id: Some(7), ..State::with_root("D:/work/demo") }
    }

    /// What people type into a preview bar is a port, not a URL.
    #[test]
    fn the_preview_bar_takes_what_a_dev_server_prints() {
        assert_eq!(normalize_url("localhost:5173").as_deref(), Some("http://localhost:5173"));
        assert_eq!(normalize_url(" 127.0.0.1:8080 ").as_deref(), Some("http://127.0.0.1:8080"));
        // Already a URL: passed through, scheme and all.
        assert_eq!(normalize_url("https://example.com").as_deref(), Some("https://example.com"));
        assert_eq!(normalize_url("about:blank").as_deref(), Some("about:blank"));
        // An empty submit must not blank the page that is up.
        assert_eq!(normalize_url("   "), None);
    }

    /// The dock draws the selected tab only while that tab still has something
    /// behind it. Closing the open file with its tab selected has to land on
    /// the terminal, not on an empty panel.
    #[test]
    fn the_dock_falls_back_to_a_tab_that_still_has_content() {
        let mut s = open_state();
        assert_eq!(s.dock_shown(), None, "no dock until something opens");

        s.reviewing = Some(("abc".into(), None));
        assert_eq!(s.dock_shown(), Some(Dock::Diff), "the only tab wins whatever is selected");

        s.viewing = Some((PathBuf::from("a.rs"), Ok(String::new())));
        s.dock = Dock::File;
        assert_eq!(s.dock_shown(), Some(Dock::File));

        s.viewing = None;
        assert_eq!(s.dock_shown(), Some(Dock::Diff), "selection gone, first live tab instead");
    }

    /// The rail and the persisted `files_open` flag are one fact; the tree is
    /// walked exactly while the Files pane is the one on screen.
    #[test]
    fn switching_the_sidebar_starts_and_stops_the_tree_walk() {
        let root = std::env::temp_dir().join("coder-pane-switch-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() {}").unwrap();
        let mut s = State::with_root(root.to_str().unwrap());

        let _ = update(&mut s, &client(), Message::SelectPane(Pane::Files));
        assert!(s.files_open, "the persisted flag follows the rail");
        assert_eq!(s.tree.len(), 1);

        let _ = update(&mut s, &client(), Message::SelectPane(Pane::Checkpoints));
        assert!(!s.files_open);
        assert!(s.tree.is_empty(), "a hidden tree is not kept warm");
    }

    #[test]
    fn a_send_needs_a_folder_and_a_prompt() {
        let mut s = State { draft: "hello".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.turns.is_empty(), "no folder open, nothing to code against");

        let mut s = State { draft: "   ".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.turns.is_empty());

        let mut s = State { draft: "hello".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(matches!(s.turns[0], Turn::User(ref t) if t == "hello"));
        assert!(s.draft.is_empty());
        assert!(s.sending);
    }

    #[test]
    fn a_tool_call_opens_a_row_and_its_result_closes_that_row() {
        let mut s = open_state();
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolCall {
                call_id: "c1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({ "path": "src/a.rs" }),
            }),
        );
        assert!(matches!(&s.turns[0], Turn::Tool { label, result: None } if label == "read_file src/a.rs"));
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "read_file".into(),
                content: "fn main() {}".into(),
            }),
        );
        assert!(
            matches!(&s.turns[0], Turn::Tool { result: Some(Ok(r)), .. } if r == "fn main() {}")
        );
    }

    /// The executor hands failures back as text the model can act on, so the
    /// `Error:` prefix is the only thing that distinguishes one — and a row
    /// showing a green tick over "File not found" is a lie.
    #[test]
    fn an_executor_error_reads_as_a_failed_row() {
        let mut s = open_state();
        tool_call(&mut s, "nope.rs");
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "read_file".into(),
                content: "Error: File not found: nope.rs".into(),
            }),
        );
        assert!(matches!(&s.turns[0], Turn::Tool { result: Some(Err(_)), .. }));
    }

    fn tool_call(state: &mut State, path: &str) {
        let _ = update(
            state,
            &client(),
            Message::Event(CoderEvent::ToolCall {
                call_id: path.into(),
                name: "read_file".into(),
                arguments: serde_json::json!({ "path": path }),
            }),
        );
    }

    /// Each answered row's text, whichever side of the `Result` it came back on.
    fn results(state: &State) -> Vec<String> {
        state
            .turns
            .iter()
            .filter_map(|t| match t {
                Turn::Tool { result: Some(r), .. } => {
                    Some(r.clone().unwrap_or_else(|e| e))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn results_fill_the_oldest_unanswered_row() {
        let mut s = open_state();
        tool_call(&mut s, "a.rs");
        tool_call(&mut s, "b.rs");
        s.resolve_tool(Ok("first".into()));
        s.resolve_tool(Ok("second".into()));
        assert_eq!(results(&s), vec!["first", "second"], "in call order, not reversed");
    }

    #[test]
    fn a_turn_that_dies_mid_call_cannot_swallow_the_next_turns_result() {
        let mut s = State { sending: true, ..open_state() };
        tool_call(&mut s, "a.rs");
        // The turn dies before that call is ever answered.
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Failed("boom".into())));
        assert_eq!(results(&s), vec!["the turn ended before this call was answered"]);

        // A fresh turn's result must land on the fresh row.
        tool_call(&mut s, "b.rs");
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "read_file".into(),
                content: "contents of b".into(),
            }),
        );
        assert_eq!(
            results(&s),
            vec!["the turn ended before this call was answered", "contents of b"]
        );
    }

    fn checkpoint(sha: &str, message: &str) -> crate::coder_git::Checkpoint {
        crate::coder_git::Checkpoint {
            sha: sha.into(),
            message: message.into(),
            when: "2 minutes ago".into(),
        }
    }

    /// Restoring is `git reset --hard`: it discards every change since that
    /// checkpoint, the user's own included. One press must not be enough.
    #[test]
    fn restoring_a_checkpoint_takes_two_presses() {
        let mut s = open_state();
        let _ = update(
            &mut s,
            &client(),
            Message::CheckpointsLoaded(Ok(vec![
                checkpoint("aaaaaaaa1111", "bump the version"),
                checkpoint("bbbbbbbb2222", "baseline"),
            ])),
        );
        assert_eq!(s.checkpoints.len(), 2);

        let _ = update(&mut s, &client(), Message::ReviewCheckpoint("aaaaaaaa1111".into()));
        assert!(matches!(&s.reviewing, Some((sha, None)) if sha == "aaaaaaaa1111"), "opens empty");
        let _ = update(&mut s, &client(), Message::DiffLoaded(Ok("+two\n-one".into())));
        assert!(matches!(&s.reviewing, Some((_, Some(d))) if d.contains("+two")));

        let _ = update(&mut s, &client(), Message::RestoreCheckpoint("aaaaaaaa1111".into()));
        assert_eq!(s.restore_armed.as_deref(), Some("aaaaaaaa1111"), "armed, not fired");
        // Looking at a different one disarms it — the second press must mean
        // the same checkpoint the first one did.
        let _ = update(&mut s, &client(), Message::ReviewCheckpoint("bbbbbbbb2222".into()));
        assert_eq!(s.restore_armed, None);

        let _ = update(&mut s, &client(), Message::RestoreCheckpoint("bbbbbbbb2222".into()));
        let _ = update(&mut s, &client(), Message::RestoreCheckpoint("bbbbbbbb2222".into()));
        assert_eq!(s.restore_armed, None, "fired, and disarmed behind itself");

        let _ = update(&mut s, &client(), Message::CloseReview);
        assert!(s.reviewing.is_none());
    }

    /// The shell is a real process in the user's folder, so the two things that
    /// matter are that it needs a folder and that closing the drawer actually
    /// ends it — `Session`'s drop sends the PTY a shutdown, and a shell still
    /// running behind a closed drawer is one nobody can reason about.
    ///
    /// This spawns a real PowerShell (ConPTY), which is the point: it is the
    /// part of the feature no state-machine test reaches.
    #[test]
    fn a_terminal_needs_a_folder_and_ends_when_the_drawer_closes() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::ToggleTerminal);
        assert!(s.term.is_none(), "no folder, nothing to open a shell in");
        let _ = update(&mut s, &client(), Message::SendToTerminal("cargo test".into()));
        assert!(s.term.is_none());

        let root = std::env::temp_dir();
        let mut s = State::with_root(root.to_str().unwrap());
        let _ = update(&mut s, &client(), Message::ToggleTerminal);
        assert!(s.term.is_some(), "a real PTY in the workspace root");

        let _ = update(&mut s, &client(), Message::ToggleTerminal);
        assert!(s.term.is_none(), "closing ends the shell rather than hiding it");
    }

    /// The widget keys its event subscription on the terminal id, so a reopened
    /// drawer that reused one would be a new PTY wired to the old subscription.
    #[test]
    fn reopening_never_reuses_a_terminal_id() {
        let mut s = State::with_root(std::env::temp_dir().to_str().unwrap());
        let _ = update(&mut s, &client(), Message::ToggleTerminal);
        let first = s.term.as_ref().unwrap().0.id;
        let _ = update(&mut s, &client(), Message::ToggleTerminal);
        let _ = update(&mut s, &client(), Message::ToggleTerminal);
        let second = s.term.as_ref().unwrap().0.id;
        assert_ne!(first, second);

        // Sending opens one if the drawer was shut, so a command never lands in
        // a terminal that is not there.
        let _ = update(&mut s, &client(), Message::ToggleTerminal);
        assert!(s.term.is_none());
        let _ = update(&mut s, &client(), Message::SendToTerminal("cargo test".into()));
        assert!(s.term.is_some());
    }

    /// The pane walks the folder itself, so this drives it against a real one.
    #[test]
    fn the_file_pane_walks_only_what_is_open() {
        let root = std::env::temp_dir().join("coder-pane-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("app.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("README.md"), "hi").unwrap();

        let mut s = State::with_root(root.to_str().unwrap());
        assert!(s.tree.is_empty(), "a closed pane walks nothing");

        let _ = update(&mut s, &client(), Message::ToggleFiles);
        let names = |s: &State| s.tree.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&s), vec!["src", "README.md"], "one level, dirs first");

        let _ = update(&mut s, &client(), Message::ToggleDir(root.join("src")));
        assert_eq!(names(&s), vec!["src", "app.rs", "README.md"], "expanded in place");

        let _ = update(&mut s, &client(), Message::OpenFile(root.join("src").join("app.rs")));
        assert!(matches!(&s.viewing, Some((_, Ok(t))) if t == "fn main() {}"));

        // Closing puts the walk away with it; the expansion survives, so
        // reopening lands where it was left.
        let _ = update(&mut s, &client(), Message::ToggleFiles);
        assert!(s.tree.is_empty());
        let _ = update(&mut s, &client(), Message::ToggleFiles);
        assert_eq!(names(&s), vec!["src", "app.rs", "README.md"]);

        let _ = update(&mut s, &client(), Message::CloseFile);
        assert!(s.viewing.is_none());
    }

    /// git may not be installed. That must read as "no history of your work",
    /// never as "the turn failed" — the turn ran either way.
    #[test]
    fn a_checkpoint_failure_is_not_a_turn_failure() {
        let mut s = open_state();
        let _ = update(
            &mut s,
            &client(),
            Message::Committed(Err("could not run git — is it installed?".into())),
        );
        assert!(s.checkpoint_error.as_deref().unwrap().contains("git"));
        assert_eq!(s.error, None, "the banner means the turn went wrong");

        // …and it clears once one lands.
        let _ = update(&mut s, &client(), Message::CheckpointsLoaded(Ok(vec![])));
        assert_eq!(s.checkpoint_error, None);
    }

    /// The plan opens the turn as an assistant row — the same row a reopened
    /// session rebuilds from the stored log — but it is not the turn's *answer*.
    /// A model that writes a plan and then dies silently is still a silent turn,
    /// and that is the failure this screen exists to name.
    #[test]
    fn a_plan_is_a_row_but_not_an_answer() {
        let mut s = State { draft: "add a test".into(), plan: true, ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Plan("1. read it".into())));
        assert!(matches!(&s.turns[1], Turn::Assistant { text, .. } if text == "1. read it"));

        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(
            s.error.as_deref().unwrap_or_default().contains("without replying"),
            "a plan is not an answer: {:?}",
            s.error
        );

        // An empty one is dropped rather than rendered as a blank bubble, the
        // same way an empty assistant message is.
        let mut s = open_state();
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Plan("  ".into())));
        assert!(s.turns.is_empty());
    }

    /// Found by running it: `llama3` (the server's resolved default) read the
    /// file, then ended the turn with an empty reply. Empty assistant messages
    /// are dropped, so the screen rendered nothing at all and read as a hang.
    #[test]
    fn a_turn_that_says_nothing_says_so() {
        let mut s = State { draft: "do a thing".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        tool_call(&mut s, "a.rs");
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "read_file".into(),
                content: "x".into(),
            }),
        );
        // The step that only calls a tool sends an empty assistant message, and
        // so does this model's final answer.
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Assistant(String::new())));
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(!s.sending);
        assert!(
            s.error.as_deref().unwrap_or_default().contains("without replying"),
            "a silent turn must not render as nothing: {:?}",
            s.error
        );

        // A turn that did answer says nothing extra.
        let mut s = State { draft: "again".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Assistant("Done.".into())));
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert_eq!(s.error, None);
    }

    #[test]
    fn switching_provider_drops_a_model_the_new_one_does_not_have() {
        use agent_platform_client::types::*;
        let entry = |id: &str, models: &[&str]| ProviderEntry {
            id: id.into(),
            label: id.into(),
            configured: true,
            local: true,
            models: ProviderModels {
                options: models.iter().map(|m| m.to_string()).collect(),
                selected_model: String::new(),
                source: "discovery".into(),
                warning: None,
                fallback_note: None,
            },
        };
        let mut s = open_state();
        let _ = update(
            &mut s,
            &client(),
            Message::CatalogLoaded(Ok(vec![
                entry("ollama", &["gemma4:latest"]),
                entry("lm_studio", &["qwen/qwen3-coder-30b"]),
            ])),
        );
        let _ = update(&mut s, &client(), Message::ModelChanged("gemma4:latest".into()));
        let _ = update(&mut s, &client(), Message::ProviderChanged("lm_studio".into()));
        assert!(s.model.is_empty(), "an ollama alias sent at LM Studio crashes llama-server");
        assert_eq!(s.model_options(), vec!["qwen/qwen3-coder-30b"]);
    }

    /// The approve route returns as soon as it pauses, so `Done` arrives right
    /// behind `approval_required`. That must not be read as a turn that ended
    /// badly — it is a turn waiting on a human.
    #[test]
    fn the_stream_closing_behind_an_approval_is_not_a_dead_turn() {
        let mut s = State { sending: true, allow_commands: true, ..open_state() };
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c1".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }),
        );
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(s.pending.is_some(), "the decision is still outstanding");
        assert_eq!(s.error, None, "waiting on a human is not a silent turn");
    }

    #[test]
    fn a_command_shows_itself_rather_than_the_tool_that_runs_it() {
        assert_eq!(
            label_for("run_command", &serde_json::json!({ "command": "cargo test" })),
            "$ cargo test"
        );
        assert_eq!(label_for("list_dir", &serde_json::json!({})), "list_dir .");
    }

    #[test]
    fn an_approval_pause_stops_the_turn_until_it_is_decided() {
        let mut s = State { sending: true, allow_commands: true, ..open_state() };
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c9".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "rm -rf ." }),
            }),
        );
        assert!(!s.sending, "nothing is in flight while it waits on a human");
        assert_eq!(s.pending.as_ref().unwrap().command, "rm -rf .");
        // No row yet: the resumed turn emits the real `tool_call` and a row
        // pushed here would sit beside it as a duplicate.
        assert!(s.turns.is_empty());
        // A send while a decision is outstanding must not open a second turn.
        s.draft = "never mind".into();
        let _ = update(&mut s, &client(), Message::Send);
        assert_eq!(s.draft, "never mind");

        let _ = update(&mut s, &client(), Message::Decide(false));
        // Still pending until the server acts on the refusal — the card is
        // hidden by `sending`, not by dropping the call.
        assert!(s.sending);
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(s.pending.is_none());
        // A refusal produces no `tool_call`, so the refusal *is* the row.
        assert!(
            matches!(&s.turns[0], Turn::Tool { label, result: Some(Err(e)) }
                if label == "$ rm -rf ." && e.contains("refused"))
        );
    }

    /// Approving is the other half: the row must come from the resumed stream,
    /// exactly once. Seen on screen as two identical `$ python …` rows before
    /// it was seen in the code.
    /// Commands off means the executor refuses anyway, so asking first is a
    /// prompt whose only possible outcome is "no" — seen on screen: approved,
    /// then refused by the other gate.
    #[test]
    fn with_commands_off_the_user_is_never_asked() {
        let mut s = State { sending: true, ..open_state() };
        assert!(!s.allow_commands);
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c9".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }),
        );
        assert!(s.pending.is_none(), "no card for a decision that is already made");
        assert!(
            matches!(&s.turns[0], Turn::Tool { label, result: Some(Err(e)) }
                if label == "$ cargo test" && e.contains("commands are off"))
        );

        // The model often says nothing after a refusal. Blaming its ability to
        // hold a tool loop would point the user at the wrong setting.
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        let banner = s.error.clone().unwrap_or_default();
        assert!(banner.contains("commands are off"), "{banner}");
        assert!(!banner.contains("tool loop"), "{banner}");
    }

    /// Seen live: a model leaked `</tool_call>` as prose, the server salvaged a
    /// `run_command` with no `command` in it, and the card offered a live Run
    /// button over an empty box.
    #[test]
    fn a_call_with_no_command_in_it_leaves_nothing_to_approve() {
        let mut s = State { sending: true, allow_commands: true, ..open_state() };
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c1".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({}),
            }),
        );
        assert_eq!(s.pending.as_ref().unwrap().command, "", "the view hides Run on this");
        assert_eq!(command_of(&serde_json::json!({ "command": "  " })), "");
        assert_eq!(command_of(&serde_json::json!({ "command": " ls " })), "ls");

        // Dismissing it is a refusal, and its row has no command to name.
        let _ = update(&mut s, &client(), Message::Decide(false));
        assert!(matches!(&s.turns[0], Turn::Tool { label, .. } if label.contains("unreadable")));
    }

    /// Seen live: the approve POST failed in transport, the client had already
    /// dropped `pending`, and the server was left holding the call — every
    /// later send came back "Thread has a command awaiting approval", with no
    /// card left to answer it from. The decision only counts once the server
    /// acts on it.
    #[test]
    fn a_decision_that_never_reached_the_server_can_be_answered_again() {
        let mut s = State { sending: true, allow_commands: true, ..open_state() };
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c9".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }),
        );
        let _ = update(&mut s, &client(), Message::Decide(true));
        assert!(s.pending.is_some(), "still outstanding until the server says otherwise");
        assert!(s.sending, "…but the card is hidden while it is in flight");

        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Failed("no route".into())));
        assert!(s.pending.is_some(), "the card comes back so it can be retried");
        assert!(!s.sending);
        assert!(s.error.as_deref().unwrap_or_default().contains("Answer it again"));

        // Retrying, and this time the server acts on it.
        let _ = update(&mut s, &client(), Message::Decide(true));
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolCall {
                call_id: "c9".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }),
        );
        assert!(s.pending.is_none(), "answered for real now");
    }

    #[test]
    fn approving_leaves_the_row_to_the_resumed_turn() {
        let mut s = State { sending: true, allow_commands: true, ..open_state() };
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c9".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }),
        );
        let _ = update(&mut s, &client(), Message::Decide(true));
        assert!(s.turns.is_empty(), "approving adds nothing on its own");

        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolCall {
                call_id: "c9".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }),
        );
        assert_eq!(s.turns.len(), 1, "one row for one command");
    }

    /// Reopening a session has to produce the same rows the live stream did.
    /// The stored log is OpenAI-shaped — tool calls hang off the assistant turn
    /// that made them, with their arguments as a JSON *string* — so this is a
    /// different parse of the same conversation, and the two drifting apart is
    /// the failure that makes history untrustworthy.
    #[test]
    fn a_reopened_session_rebuilds_the_rows_the_stream_produced() {
        let stored = serde_json::json!([
            { "role": "system", "content": "You are a coding assistant" },
            { "role": "user", "content": "add farewell" },
            {
                "role": "assistant",
                "content": "Let me read it first.",
                "tool_calls": [{
                    "id": "c1",
                    "function": { "name": "read_file", "arguments": "{\"path\":\"src/a.py\"}" }
                }]
            },
            { "role": "tool", "tool_call_id": "c1", "name": "read_file", "content": "def greet(): ..." },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "c2",
                    "function": { "name": "write_file", "arguments": "{\"path\":\"src/a.py\"}" }
                }]
            },
            { "role": "tool", "tool_call_id": "c2", "name": "write_file", "content": "Wrote 40 bytes to src/a.py" },
            { "role": "assistant", "content": "Done." },
        ]);
        let turns = rebuild_turns(stored.as_array().unwrap());

        assert!(matches!(&turns[0], Turn::User(t) if t == "add farewell"));
        assert!(matches!(&turns[1], Turn::Assistant { text, .. } if text == "Let me read it first."));
        assert!(matches!(&turns[2], Turn::Tool { label, result: Some(Ok(r)) }
            if label == "read_file src/a.py" && r.starts_with("def greet")));
        // The tool-only step has empty content and must not leave a blank bubble.
        assert!(matches!(&turns[3], Turn::Tool { label, result: Some(Ok(_)) }
            if label == "write_file src/a.py"));
        assert!(matches!(&turns[4], Turn::Assistant { text, .. } if text == "Done."));
        assert_eq!(turns.len(), 5, "no system row, no blank assistant row");
    }

    #[test]
    fn a_session_that_ended_mid_call_reopens_with_that_row_closed() {
        let stored = serde_json::json!([
            { "role": "user", "content": "go" },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "c1",
                    "function": { "name": "run_command", "arguments": "{\"command\":\"pytest\"}" }
                }]
            },
        ]);
        let turns = rebuild_turns(stored.as_array().unwrap());
        assert!(matches!(&turns[1], Turn::Tool { label, result: Some(Err(e)) }
            if label == "$ pytest" && e.contains("never answered")));
    }

    /// Ported from hearth's composer: most specific first. A bare "thinking" for
    /// two minutes is how a wait on the approval gate reads as a hang.
    #[test]
    fn the_status_line_names_what_is_actually_being_waited_on() {
        let mut s = State { sending: true, ..open_state() };
        assert_eq!(s.activity(), "thinking");

        tool_call(&mut s, "src/big.rs");
        assert_eq!(s.activity(), "read_file src/big.rs", "the tool in flight beats the model");

        s.resolve_tool(Ok("...".into()));
        assert_eq!(s.activity(), "thinking", "answered calls stop counting");

        // The gate is only a wait on the user while nothing is in flight — once
        // the decision is sent, `pending` survives but the wait is the server's.
        s.sending = false;
        s.pending = Some(Pending { call_id: "c1".into(), command: "pytest".into() });
        assert_eq!(s.activity(), "waiting for you", "the user beats everything");
        s.sending = true;
        assert_eq!(s.activity(), "thinking", "a decision in flight is not a wait on the user");
    }

    #[test]
    fn the_clock_only_runs_while_a_turn_does() {
        let mut s = State { draft: "go".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        for _ in 0..3 {
            let _ = update(&mut s, &client(), Message::Tick);
        }
        assert_eq!(s.elapsed, 3);
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        let _ = update(&mut s, &client(), Message::Tick);
        assert_eq!(s.elapsed, 3, "a finished turn stops counting");

        // But a turn parked on the gate has not finished, and that wait is the
        // longest one available here — the counter froze on it once.
        s.pending = Some(Pending { call_id: "c1".into(), command: "pytest".into() });
        let _ = update(&mut s, &client(), Message::Tick);
        assert_eq!(s.elapsed, 4, "the clock runs while it waits on a human");
        s.pending = None;

        // The next turn starts from zero rather than continuing the last one.
        s.draft = "again".into();
        let _ = update(&mut s, &client(), Message::Send);
        assert_eq!(s.elapsed, 0);
    }

    #[test]
    fn reopening_a_session_follows_its_folder_and_thread() {
        use agent_platform_client::types::CoderThreadOut;
        let mut s = State::with_root("D:/work/first");
        let _ = update(
            &mut s,
            &client(),
            Message::ThreadLoaded(Ok(Box::new(CoderThreadOut {
                thread_id: 42,
                title: "old one".into(),
                workspace_root: Some("D:/work/second".into()),
                messages: vec![serde_json::json!({ "role": "user", "content": "hi" })],
            }))),
        );
        assert_eq!(s.thread_id, Some(42));
        assert_eq!(s.root, Some(PathBuf::from("D:/work/second")), "the root travels with it");
        assert_eq!(s.turns.len(), 1);

        // Deleting the open session detaches it, so the next send opens a new
        // thread rather than writing into one the server no longer has.
        s.threads = vec![];
        let _ = update(&mut s, &client(), Message::DeleteThread(42));
        assert_eq!(s.thread_id, None);
    }

    /// The delegation round trip, against a live server and a real model.
    ///
    /// The unit tests above prove the state machine; none of them prove the
    /// protocol, which is the part with two processes in it — the server has to
    /// actually block on `(thread_id, call_id)`, and `coder_tool_result` has to
    /// actually unblock it. Everything here is the shipped code: the real
    /// stream, the real executor, the real client.
    ///
    /// `#[ignore]` because it needs a running platform and a tool-capable model.
    /// Run it with:
    ///
    /// ```text
    /// cargo test -p agent-platform-desktop -- --ignored --nocapture delegation
    /// ```
    ///
    /// Override the model with `CODER_TEST_MODEL` / `CODER_TEST_PROVIDER`. The
    /// defaults are what this machine actually serves — note that the alias has
    /// to be one the *routed* provider can load: an Ollama-style
    /// `gemma4:latest` sent at an LM Studio backend crashed llama-server rather
    /// than 404ing, which is how that got pinned down.
    #[tokio::test]
    #[ignore = "needs a running agent-platform and a tool-capable local model"]
    async fn delegation_round_trip_edits_a_real_file() {
        use futures::StreamExt;

        let key = std::fs::read_to_string(
            dirs::config_dir().unwrap().join(crate::shell::APP_DIR).join("master.key"),
        )
        .expect("no install key — start the desktop app once");
        let base = std::env::var("AGENT_PLATFORM_BASE")
            .unwrap_or_else(|_| "http://127.0.0.1:18410".into());
        let model = std::env::var("CODER_TEST_MODEL")
            .unwrap_or_else(|_| "qwen/qwen3-coder-30b".to_string());
        let provider =
            std::env::var("CODER_TEST_PROVIDER").unwrap_or_else(|_| "lm_studio".to_string());
        let client = Client::new(base, key.trim());

        let root = std::env::temp_dir().join("coder-delegation-e2e");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::write(root.join("greeting.txt"), "hello\n").unwrap();

        let thread = client
            .create_coder_thread(&root.display().to_string())
            .await
            .expect("could not open a coder thread")
            .thread_id;

        let body = serde_json::json!({
            "message": "Edit greeting.txt so it says goodbye instead of hello. \
                        Read it first, then write it back.",
            "thread_id": thread,
            "workspace_root": root.display().to_string(),
            "allow_commands": false,
            "auto_approve_commands": false,
            "delegate_tools": true,
            "model": model,
            "provider": provider,
        });

        let mut calls: Vec<String> = Vec::new();
        let drive = async {
            let mut stream =
                Box::pin(coder_stream(client.clone(), "/api/v1/coder/chat/stream", body));
            while let Some(event) = stream.next().await {
                match event {
                    CoderEvent::ToolCall { call_id, name, arguments } => {
                        calls.push(name.clone());
                        let result =
                            crate::coder_tools::execute(&root, &name, &arguments, false).await;
                        // The server is parked on this. If the POST fails the
                        // turn stalls, so a failure here is the test failing.
                        client
                            .coder_tool_result(thread, &call_id, &result)
                            .await
                            .expect("handing the tool result back failed");
                    }
                    CoderEvent::Failed(e) => panic!("stream failed: {e}"),
                    CoderEvent::Done => break,
                    _ => {}
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(300), drive)
            .await
            .expect("the turn did not finish in 300s");

        // The trace is the useful half when this fails: a model that never
        // called a tool and one whose edit was refused look identical from the
        // file alone.
        eprintln!("[{model}] trace: {}", calls.join(" → "));
        let after = std::fs::read_to_string(root.join("greeting.txt")).unwrap();
        assert!(
            calls.iter().any(|c| c == "write_file"),
            "the model never wrote anything; calls were {calls:?}"
        );
        assert!(
            after.to_lowercase().contains("goodbye"),
            "the edit never reached disk. calls: {calls:?}, file: {after:?}"
        );
    }

    #[test]
    fn changing_folder_starts_a_new_thread_but_keeps_the_command_setting() {
        let mut s = State { allow_commands: true, ..open_state() };
        s.turns.push(Turn::User("old".into()));
        let _ = update(&mut s, &client(), Message::RootPicked(Some("D:/work/other".into())));
        assert_eq!(s.thread_id, None, "the old thread names the old workspace root");
        assert!(s.turns.is_empty());
        assert!(s.allow_commands);
        assert_eq!(s.root, Some(PathBuf::from("D:/work/other")));
    }
}
