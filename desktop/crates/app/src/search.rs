//! Web search: turn a sentence into a Google dork and hand it to the browser
//! (ADR 0008, `docs/web-search-module-plan.md`). State and `update` only —
//! rendering is `search_view.rs`, per the root `CLAUDE.md` split.
//!
//! The server does the actual translation and the actual rendering (`GET
//! /api/v1/search`). Removing a chip, or adding an operator, is not a local
//! edit: it sends the current query back with `drop=<token>` or
//! `add_field=<field>&add_value=<value>` and lets the response —
//! `dork.query`, `dork.explanation`, `dork.chips` — become the new truth,
//! same as every other path through this screen. That is what keeps the
//! operator grammar (field order, quoting, negation) in exactly one place,
//! `server/src/search_dork.rs`; this crate never re-renders a dork.
//!
//! **Results are gated on `configured`, not on `results` being non-empty.**
//! `configured: false` is the default install and means "this install does
//! not do results", not "no matches" — see the ADR's amendment and
//! `search_view.rs`'s rendering of the two cases.
//!
//! **History replaces the old in-memory recents.** The server owns it now
//! (`/api/v1/search/history`); this screen loads it on entry and posts to it
//! at exactly two points — see [`Message::Fetched`] and [`Message::OpenResult`]
//! — never on a chip removal or an operator add, which would fill it with
//! fragments of a query nobody ran.

use crate::domain::err_string;
use agent_platform_client::types::{SearchEngine, SearchHistoryEntry, SearchResponse};
use agent_platform_client::{Client, DorkRequest};
use iced::Task;

/// Which box the next Run submits. Typing in the sentence box arms `Ask`;
/// typing in the dork box (or removing a chip, or adding an operator, both of
/// which edit it programmatically) arms `Query`. The label shown beside Run is
/// this flag in words — the "make that switch visible" the plan asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ask,
    Query,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Ask
    }
}

/// The add-operator picker (`docs/web-search-module-plan.md`'s Part 2 update):
/// plain English first so it can be found without knowing dork syntax, the
/// operator second so the user leaves knowing it. `wire()` is the
/// `DorkQuery::add_part` field name this sends as `add_field=` — never an
/// operator spelling; the server alone renders those
/// (`server/src/search_dork.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddField {
    Sites,
    ExcludeSites,
    Intitle,
    Intext,
    Inurl,
    Filetype,
    Exact,
    Exclude,
    Related,
    Range,
    After,
    Before,
}

impl AddField {
    pub const ALL: [AddField; 12] = [
        AddField::Sites,
        AddField::ExcludeSites,
        AddField::Intitle,
        AddField::Intext,
        AddField::Inurl,
        AddField::Filetype,
        AddField::Exact,
        AddField::Exclude,
        AddField::Related,
        AddField::Range,
        AddField::After,
        AddField::Before,
    ];

    /// The `DorkQuery` field name this maps to server-side — see
    /// `search_dork.rs::DorkQuery::add_part`'s match arms.
    pub fn wire(self) -> &'static str {
        match self {
            AddField::Sites => "sites",
            AddField::ExcludeSites => "exclude_sites",
            AddField::Intitle => "intitle",
            AddField::Intext => "intext",
            AddField::Inurl => "inurl",
            AddField::Filetype => "filetype",
            AddField::Exact => "exact",
            AddField::Exclude => "exclude",
            AddField::Related => "related",
            AddField::Range => "range",
            AddField::After => "after",
            AddField::Before => "before",
        }
    }

    /// A hint for the value input beside this field in the picker.
    pub fn placeholder(self) -> &'static str {
        match self {
            AddField::Sites | AddField::ExcludeSites | AddField::Related => "example.com",
            AddField::Intitle | AddField::Intext | AddField::Inurl | AddField::Exact => {
                "a phrase"
            }
            AddField::Filetype => "pdf",
            AddField::Exclude => "a word to exclude",
            AddField::Range => "100..200",
            AddField::After | AddField::Before => "YYYY-MM-DD",
        }
    }
}

impl Default for AddField {
    fn default() -> Self {
        AddField::Sites
    }
}

/// The picker's label — both halves, per the plan's table: plain English so
/// it is findable without knowing the syntax, the operator so the user leaves
/// knowing it.
impl std::fmt::Display for AddField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            AddField::Sites => "Only on this site (site:)",
            AddField::ExcludeSites => "Not on this site (-site:)",
            AddField::Intitle => "Page title contains (intitle:)",
            AddField::Intext => "Page text contains (intext:)",
            AddField::Inurl => "Page address contains (inurl:)",
            AddField::Filetype => "File type (filetype:)",
            AddField::Exact => "Exact phrase (\"…\")",
            AddField::Exclude => "Exclude word (-)",
            AddField::Related => "Similar sites to (related:)",
            AddField::Range => "Number between (..)",
            AddField::After => "Published after (after:)",
            AddField::Before => "Published before (before:)",
        };
        write!(f, "{label}")
    }
}

#[derive(Default)]
pub struct State {
    pub ask: String,
    /// The rendered query, editable. Always what the Query box shows.
    pub query_text: String,
    pub mode: Mode,
    pub engine: SearchEngine,
    /// The last full response — display only, so it is allowed to lag one
    /// edit behind `query_text` until the next Run lands.
    pub response: Option<SearchResponse>,
    pub history: Vec<SearchHistoryEntry>,
    pub add_field: AddField,
    pub add_value: String,
    /// Set right before a fetch that must be recorded as `opened: false` on
    /// success — an Ask submission, and only an Ask submission (see this
    /// module's doc comment). Consumed the moment [`Message::Fetched`] lands.
    pending_history: bool,
    /// The unconfigured-install hint, dismissed for the rest of the session
    /// once the user closes it.
    pub hint_dismissed: bool,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    AskChanged(String),
    /// The dork box was hand-typed.
    QueryChanged(String),
    /// Enter in either box, or the Search/Run button: submits whichever box
    /// [`Mode`] currently points at.
    Run,
    /// One chip's `×` — carries that chip's own token, exactly as the server
    /// rendered it (`site:reddit.com`, `-membrane`, …), which is what
    /// `search_view.rs::part_chips` builds each chip's label from.
    RemovePart(String),
    EngineChanged(SearchEngine),
    AddFieldChanged(AddField),
    AddValueChanged(String),
    /// The add-operator row's Add button.
    AddOperator,
    Fetched(Result<Box<SearchResponse>, String>),
    /// A result row, or "Open in <engine>" — both open a URL in the real
    /// browser (`crate::shell::open_url`); only the latter records history.
    OpenLink(String),
    OpenResult,
    LoadHistory,
    HistoryLoaded(Result<Vec<SearchHistoryEntry>, String>),
    /// A history row: reruns its query verbatim.
    HistorySelected(i64),
    HistoryDelete(i64),
    HistoryClear,
    /// Shared by every history write (record, delete, clear): reload the list
    /// either way, same as `workflows.rs::Message::Mutated`.
    HistoryMutated(Result<(), String>),
    DismissHint,
    Dismiss,
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::AskChanged(v) => {
            state.ask = v;
            state.mode = Mode::Ask;
            Task::none()
        }
        Message::QueryChanged(v) => {
            state.query_text = v;
            state.mode = Mode::Query;
            Task::none()
        }
        Message::Run => match state.mode {
            Mode::Ask => run_ask(state, client),
            Mode::Query => run_search(state, client, None, None, None),
        },
        Message::RemovePart(token) => {
            state.mode = Mode::Query;
            run_search(state, client, Some(&token), None, None)
        }
        Message::EngineChanged(engine) => {
            state.engine = engine;
            if state.query_text.trim().is_empty() {
                Task::none()
            } else {
                run_search(state, client, None, None, None)
            }
        }
        Message::AddFieldChanged(f) => {
            state.add_field = f;
            Task::none()
        }
        Message::AddValueChanged(v) => {
            state.add_value = v;
            Task::none()
        }
        Message::AddOperator => {
            state.mode = Mode::Query;
            let field = state.add_field.wire();
            let value = state.add_value.trim().to_string();
            run_search(state, client, None, Some(field), Some(&value))
        }
        Message::Fetched(Ok(response)) => {
            state.busy = false;
            state.error = None;
            state.query_text = response.dork.query.clone();
            state.engine = response.dork.engine;
            state.mode = Mode::Query;
            state.add_value.clear();
            let record = std::mem::take(&mut state.pending_history);
            let (query, engine, source) = (
                response.dork.query.clone(),
                response.dork.engine.as_str().to_string(),
                response.dork.source.clone(),
            );
            state.response = Some(*response);
            if record {
                record_history(client, query, engine, source, false)
            } else {
                Task::none()
            }
        }
        Message::Fetched(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Message::OpenLink(url) => {
            crate::shell::open_url(&url);
            Task::none()
        }
        Message::OpenResult => match &state.response {
            Some(r) => {
                crate::shell::open_url(&r.dork.url);
                record_history(
                    client,
                    r.dork.query.clone(),
                    r.dork.engine.as_str().to_string(),
                    r.dork.source.clone(),
                    true,
                )
            }
            None => Task::none(),
        },
        Message::LoadHistory => refresh_history(client),
        Message::HistoryLoaded(Ok(items)) => {
            state.history = items;
            state.error = None;
            Task::none()
        }
        Message::HistoryLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::HistorySelected(id) => {
            match state.history.iter().find(|h| h.id == id).map(|h| h.query.clone()) {
                Some(q) => {
                    state.query_text = q;
                    state.mode = Mode::Query;
                    run_search(state, client, None, None, None)
                }
                None => Task::none(),
            }
        }
        Message::HistoryDelete(id) => {
            let client = client.clone();
            Task::perform(
                async move { err_string(client.delete_search_history(id).await) },
                Message::HistoryMutated,
            )
        }
        Message::HistoryClear => {
            let client = client.clone();
            Task::perform(
                async move { err_string(client.clear_search_history().await) },
                Message::HistoryMutated,
            )
        }
        Message::HistoryMutated(result) => {
            if let Err(e) = result {
                state.error = Some(e);
            }
            refresh_history(client)
        }
        Message::DismissHint => {
            state.hint_dismissed = true;
            Task::none()
        }
        Message::Dismiss => {
            state.error = None;
            Task::none()
        }
    }
}

fn run_ask(state: &mut State, client: &Client) -> Task<Message> {
    let ask = state.ask.trim().to_string();
    if ask.is_empty() {
        state.error = Some("Type a sentence to search for.".into());
        return Task::none();
    }
    state.busy = true;
    state.error = None;
    state.pending_history = true;
    let (client, engine) = (client.clone(), state.engine);
    Task::perform(
        async move {
            let req = DorkRequest { ask: Some(&ask), engine, ..Default::default() };
            err_string(client.search(req, None).await).map(Box::new)
        },
        Message::Fetched,
    )
}

/// Every other run through this screen: a verbatim Run of the query box, a
/// chip removal (`drop`), or an operator add (`add_field`/`add_value`). None
/// of these is "built from a submitted sentence", so `pending_history` is
/// always cleared here, never set.
fn run_search(
    state: &mut State,
    client: &Client,
    drop: Option<&str>,
    add_field: Option<&str>,
    add_value: Option<&str>,
) -> Task<Message> {
    let q = state.query_text.trim().to_string();
    if q.is_empty() {
        state.error = Some("Nothing to run — type a query, or a sentence above.".into());
        return Task::none();
    }
    state.busy = true;
    state.error = None;
    state.pending_history = false;
    let (client, engine) = (client.clone(), state.engine);
    let (drop, add_field, add_value) =
        (drop.map(str::to_string), add_field.map(str::to_string), add_value.map(str::to_string));
    Task::perform(
        async move {
            let req = DorkRequest {
                q: Some(&q),
                engine,
                drop: drop.as_deref(),
                add_field: add_field.as_deref(),
                add_value: add_value.as_deref(),
                ..Default::default()
            };
            err_string(client.search(req, None).await).map(Box::new)
        },
        Message::Fetched,
    )
}

fn refresh_history(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.search_history(None, false).await).map(|r| r.history) },
        Message::HistoryLoaded,
    )
}

fn record_history(
    client: &Client,
    query: String,
    engine: String,
    source: String,
    opened: bool,
) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            err_string(client.create_search_history(&query, &engine, &source, opened).await)
                .map(|_| ())
        },
        Message::HistoryMutated,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn response(query: &str) -> SearchResponse {
        serde_json::from_value(json!({
            "query": query, "url": "https://www.google.com/search?q=x", "engine": "google",
            "source": "rules", "recipes": [], "parts": {"terms": query}, "explanation": [],
            "chips": [], "configured": false, "results": [], "total_estimate": null
        }))
        .unwrap()
    }

    #[test]
    fn typing_in_either_box_arms_the_mode_it_belongs_to() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::AskChanged("cheap keyboards".into()));
        assert_eq!(s.mode, Mode::Ask);
        let _ = update(&mut s, &client(), Message::QueryChanged("keyboard site:reddit.com".into()));
        assert_eq!(s.mode, Mode::Query);
        // Typing in the sentence box again re-arms ask mode, even after an edit.
        let _ = update(&mut s, &client(), Message::AskChanged("cheap keyboards again".into()));
        assert_eq!(s.mode, Mode::Ask);
    }

    /// Removing a chip must not edit `parts`/`query_text` itself — it reruns
    /// the current query with `drop=<token>` and waits for the response to
    /// become the new truth, same as every other path through this screen
    /// (`Message::Fetched` is what actually updates `parts`/`query_text`).
    #[test]
    fn removing_a_chip_reruns_the_query_with_drop_instead_of_editing_locally() {
        let mut s = State::default();
        let _ = update(
            &mut s,
            &client(),
            Message::Fetched(Ok(Box::new(response("keyboard site:reddit.com -membrane")))),
        );

        let _ = update(&mut s, &client(), Message::RemovePart("site:reddit.com".into()));

        assert_eq!(s.query_text, "keyboard site:reddit.com -membrane");
        assert_eq!(s.mode, Mode::Query);
        assert!(s.busy);
        assert!(s.error.is_none());
    }

    #[test]
    fn a_successful_fetch_clears_a_stale_error() {
        let mut s = State::default();
        s.error = Some("boom".into());
        let _ = update(&mut s, &client(), Message::Fetched(Ok(Box::new(response("x")))));
        assert!(s.error.is_none());
    }

    #[test]
    fn an_empty_ask_or_query_is_rejected_without_a_network_call() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::Run);
        assert!(s.error.is_some());
        assert!(!s.busy);
    }

    /// The one place history gets a `built` write — an Ask submission — sets
    /// the flag that pays off once the response lands.
    #[test]
    fn an_ask_submission_arms_pending_history_but_a_query_run_does_not() {
        let mut s = State::default();
        s.ask = "cheap keyboards".into();
        let _ = update(&mut s, &client(), Message::Run);
        assert!(s.pending_history, "ask=… run must be recorded once it resolves");

        let mut s2 = State::default();
        s2.query_text = "keyboard".into();
        s2.mode = Mode::Query;
        let _ = update(&mut s2, &client(), Message::Run);
        assert!(!s2.pending_history, "a verbatim query run is not \"built from a sentence\"");
    }

    /// Chip removal and operator-add both hit the same route as a real
    /// submission, but neither is a query "built from a submitted sentence" —
    /// recording either would fill history with fragments nobody ran.
    #[test]
    fn chip_removal_and_operator_add_never_arm_pending_history() {
        let mut s = State::default();
        s.query_text = "keyboard site:reddit.com".into();
        let _ = update(&mut s, &client(), Message::RemovePart("site:reddit.com".into()));
        assert!(!s.pending_history);

        let mut s2 = State::default();
        s2.query_text = "keyboard".into();
        s2.add_value = "reddit.com".into();
        let _ = update(&mut s2, &client(), Message::AddOperator);
        assert!(!s2.pending_history);
    }

    #[test]
    fn add_field_wire_names_match_the_servers_add_part_fields() {
        assert_eq!(AddField::Sites.wire(), "sites");
        assert_eq!(AddField::ExcludeSites.wire(), "exclude_sites");
        assert_eq!(AddField::Intitle.wire(), "intitle");
        assert_eq!(AddField::Intext.wire(), "intext");
        assert_eq!(AddField::Inurl.wire(), "inurl");
        assert_eq!(AddField::Filetype.wire(), "filetype");
        assert_eq!(AddField::Exact.wire(), "exact");
        assert_eq!(AddField::Exclude.wire(), "exclude");
        assert_eq!(AddField::Related.wire(), "related");
        assert_eq!(AddField::Range.wire(), "range");
        assert_eq!(AddField::After.wire(), "after");
        assert_eq!(AddField::Before.wire(), "before");
    }

    #[test]
    fn history_loaded_replaces_the_list_and_clears_a_stale_error() {
        let mut s = State::default();
        s.error = Some("boom".into());
        let entry: SearchHistoryEntry = serde_json::from_value(json!({
            "id": 1, "workspace_id": null, "query": "keyboard", "engine": "google",
            "source": "verbatim", "opened": true, "created_at": "2026-08-15T00:00:00"
        }))
        .unwrap();
        let _ = update(&mut s, &client(), Message::HistoryLoaded(Ok(vec![entry])));
        assert_eq!(s.history.len(), 1);
        assert!(s.error.is_none());
    }
}
