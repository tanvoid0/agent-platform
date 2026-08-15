//! E.V.'s hands on the rest of the app: read anything the REST API serves, and
//! move the user to the screen that shows it.
//!
//! Shaped exactly like [`crate::memory`]'s toolkit — a `TOOLS` list the
//! assistant checks before dispatching, a `tools_spec` half, and a runner — so
//! adding a third toolkit later is a third module and not a rewrite of
//! `assistant.rs`.
//!
//! **Reads are one tool, not twelve.** `api_get` takes a path, because the
//! server already has a REST surface for every screen and a tool per route
//! would be a second, worse copy of it that goes stale the moment a route
//! moves. The description below lists the paths worth knowing; the guard that
//! keeps it read-only is in `Client::api_get`.
//!
//! **Writes and shell commands never run from here.** `api_write` and
//! `run_command` come back as a [`Pending`], which `assistant.rs` holds and
//! `assistant_view::approval` renders as a card. Nothing reaches the network or
//! a shell until the user presses Run. There is no allowlist of "safe" calls
//! behind that — the card is the check.

use crate::Screen;
use agent_platform_client::types::SearchEngine;
use agent_platform_client::Client;

/// Tool names handled here. The assistant checks this before dispatching a call
/// to the terminal.
pub const TOOLS: [&str; 4] = ["api_get", "api_write", "open_screen", "web_search"];

/// Handled without leaving the update loop, against the app's own state rather
/// than the network. `api_write` and `web_search` are in here because parking
/// is their synchronous answer — each reaches the network only after the user
/// says so.
pub const SYNC_TOOLS: [&str; 3] = ["open_screen", "api_write", "web_search"];

/// Something the model asked to do that nobody has agreed to yet. Held on
/// `assistant::State`, rendered as a confirm card, and only then run.
///
/// Every variant is "this leaves the app and changes something outside it",
/// which is the line the card draws. Reads (`api_get`, `list_memories`) never
/// come through here — a confirmation for every one of those is a confirmation
/// nobody reads, and then the one that mattered gets clicked through too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// A REST write against this app's own API.
    Write {
        /// `tool_call_id` this answers once it is run or refused.
        id: String,
        method: String,
        path: String,
        /// Request body as the model wrote it. Shown verbatim on the card — a
        /// summary here would be the thing the user is agreeing to being
        /// different from the thing that gets sent.
        body: serde_json::Value,
    },
    /// A shell command on the user's own machine. Nothing bounds what this can
    /// do, which is exactly why it is on this list: the persona asking the model
    /// not to be destructive is a request, not a guard.
    Command { id: String, command: String },
    /// A web search, waiting to be opened in the user's real browser
    /// (`shell::open_url`) — see ADR 0008 decision 7. Only the query and the
    /// engine are known here; the URL itself is only resolvable by calling
    /// `GET /api/v1/search/dork`, which happens on approval.
    Search {
        id: String,
        /// The dork the model wrote, or the sentence it asked — whichever was
        /// given. Shown verbatim on the card, same reasoning as `Write`'s body.
        query: String,
        /// `true` when `query` is a dork already written (`q`, the form E.V.
        /// is told to prefer); `false` when it is a sentence to translate
        /// (`ask`). Decides which parameter `Client::search_dork` gets on
        /// approval — sending a sentence as `q` would skip the server's
        /// recipe/model translation entirely.
        verbatim: bool,
        engine: SearchEngine,
    },
}

impl Pending {
    pub fn id(&self) -> &str {
        match self {
            Pending::Write { id, .. } | Pending::Command { id, .. } | Pending::Search { id, .. } => id,
        }
    }

    /// The card's question. Different words because they are different risks:
    /// this app's data, the whole machine, or the user's browser.
    pub fn heading(&self) -> &'static str {
        match self {
            Pending::Write { .. } => "Make this change?",
            Pending::Command { .. } => "Run this on your machine?",
            Pending::Search { .. } => "Open this search in your browser?",
        }
    }

    /// One line: exactly what happens, minus any body.
    pub fn summary(&self) -> String {
        match self {
            Pending::Write { method, path, .. } => format!("{method} {path}"),
            Pending::Command { command, .. } => format!("$ {command}"),
            // Named engine, not the URL: the URL does not exist yet (it is
            // built server-side on approval), and a percent-encoded guess at
            // it would be less honest than what is actually known right now.
            Pending::Search { query, engine, .. } => format!("Search {engine} for: {query}"),
        }
    }

    /// The rest of it, when there is a rest. `{}` is not worth a code block.
    pub fn detail(&self) -> Option<String> {
        match self {
            Pending::Write { body, .. } if *body != serde_json::json!({}) => {
                Some(serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string()))
            }
            // The command/search are the whole of it — `summary` already says
            // everything there is to agree to.
            _ => None,
        }
    }
}

/// Screens `open_screen` will move to, and what they hold. Doubles as the
/// enum's allowed-values list in the tool spec, so the model cannot ask for a
/// screen that does not exist.
const SCREENS: [(&str, Screen, &str); 13] = [
    ("dashboard", Screen::Dashboard, "server health and the landing page"),
    ("processes", Screen::Processes, "agent runs, live and past"),
    ("projects", Screen::Projects, "projects"),
    ("teams", Screen::Teams, "team templates and their rosters"),
    ("workflows", Screen::Workflows, "saved workflows and their runs"),
    ("plans", Screen::Plans, "the todo boards"),
    ("agenda", Screen::Agenda, "the personal assistant's day/week/month board"),
    ("coder", Screen::Coder, "the coding agent, its files and its terminal"),
    ("assistant", Screen::Assistant, "this conversation"),
    ("memory", Screen::Memory, "what E.V. remembers about the user"),
    ("logs", Screen::Logs, "the server log"),
    ("settings", Screen::Settings, "theme, voice, providers, model-ops"),
    ("search", Screen::Search, "web search query builder"),
];

pub fn screen_named(name: &str) -> Option<Screen> {
    let want = name.trim().to_lowercase();
    SCREENS.iter().find(|(n, _, _)| *n == want).map(|(_, s, _)| *s)
}

/// The app half of the assistant's tool spec, in OpenAI function form.
pub fn tools_spec() -> Vec<serde_json::Value> {
    let screens: Vec<&str> = SCREENS.iter().map(|(n, _, _)| *n).collect();
    let screen_help = SCREENS
        .iter()
        .map(|(n, _, what)| format!("{n} ({what})"))
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "api_get",
                "description": "Read this app's own data over its REST API and return the raw \
                    JSON. Use this instead of guessing or asking the user. Useful paths: \
                    /api/v1/teams/ (teams; trailing slash required), \
                    /api/v1/projects/ (projects; trailing slash required), \
                    /api/v1/workflows (workflows), \
                    /api/v1/workflows/{id}/runs?limit=20 (one workflow's run history), \
                    /api/v1/todos/boards (plan boards), \
                    /api/v1/todos/boards/{id} (one board with its items), \
                    /api/v1/assistant/dashboard?project_id={id}&horizon=day (today's agenda; \
                    horizon is day, week or month, and project_id comes from /api/v1/projects/), \
                    /api/v1/processes?limit=20&unassigned_only=true (agent runs; this route \
                    rejects a bare limit — it needs one of project_id, client_id or \
                    unassigned_only=true), \
                    /api/v1/processes/{id} (one run with its tasks), \
                    /api/v1/system/status (server health), \
                    /api/v1/search/dork?q=… (build a Google-dork web search query; q takes \
                    dork operators — site:, filetype:, intitle:, \"exact phrase\", -exclude — \
                    directly and is the form to prefer, since you can already write the \
                    operators yourself and do not need the server to translate a sentence for \
                    you; ask=… is the sentence-translating alternative). \
                    Only GET, only /api/v1/ — this cannot change anything.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path with query string, starting with /api/v1/."
                        }
                    },
                    "required": ["path"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "api_write",
                "description": "Ask to change something in this app: create, update, delete or \
                    run. The user sees the method, path and body and has to approve it before \
                    anything happens, so propose the call rather than asking them in prose — but \
                    never claim it is done until the tool result says so. One change per turn. \
                    Useful calls: POST /api/v1/todos/boards/{board}/items {\"title\": \"…\"} \
                    (add a plan item), PATCH /api/v1/todos/items/{id} {\"status\": \"done\"} \
                    (change one), POST /api/v1/assistant/items/{id}/complete {} (log a \
                    completion on the agenda), POST /api/v1/workflows/{id}/run {\"input\": {}} \
                    (run a workflow), POST /api/v1/projects/ {\"name\": \"…\"}, \
                    POST /api/v1/teams/ {\"name\": \"…\", \"roster\": {…}}. \
                    Read the matching api_get first when you need an id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "method": { "type": "string", "enum": ["POST", "PATCH", "PUT", "DELETE"] },
                        "path": {
                            "type": "string",
                            "description": "Path starting with /api/v1/."
                        },
                        "body": {
                            "type": "object",
                            "description": "JSON request body. {} when the route takes none."
                        }
                    },
                    "required": ["method", "path"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "open_screen",
                "description": format!(
                    "Move the user to a screen in this app. Use it when they ask to be taken \
                     somewhere, or alongside an answer when the screen shows more than you can \
                     say. Screens: {screen_help}."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "screen": { "type": "string", "enum": screens }
                    },
                    "required": ["screen"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Open a web search in the user's real browser — this leaves the \
                    app, so the user sees the query and the engine on a confirm card and has to \
                    approve it before anything opens. Prefer q: a Google-dork query you write \
                    yourself with operators (site:, filetype:, intitle:, \"exact phrase\", \
                    -exclude) — you are already a language model, so writing the operators \
                    yourself skips a translation round trip. Use ask instead only when you want \
                    the server to translate a plain sentence for you.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "q": {
                            "type": "string",
                            "description": "A dork you already wrote, e.g. filetype:pdf intitle:\"Attention Is All You Need\"."
                        },
                        "ask": {
                            "type": "string",
                            "description": "A plain sentence to translate, when you are not writing the dork yourself."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["google", "duckduckgo", "bing"],
                            "description": "Defaults to google."
                        }
                    },
                    "required": []
                }
            }
        }),
    ]
}

/// What answering a tool inside the update loop produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sync {
    /// Answered outright; this is the tool result.
    Answered(String),
    /// Answered, and the shell should move here — this module cannot navigate
    /// on its own, and a tool that silently did would be a screen change with
    /// no record in the transcript.
    Navigated(String, Screen),
    /// Not answered at all yet: the user has to agree first. The turn stalls
    /// here until they do.
    Parked(Box<Pending>),
}

/// Answer the tools that need nothing but the app itself. `None` means "not one
/// of mine", matching [`crate::memory::run_tool`].
///
/// `run_command` is parked here too when the user has confirmation on, even
/// though the terminal itself lives in `assistant.rs` — parking is a decision
/// about consent, and all of it belongs in one place.
pub fn run_sync_tool(id: &str, name: &str, arguments: &str) -> Option<Sync> {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let string = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match name {
        "open_screen" => {
            let asked = string("screen");
            Some(match screen_named(&asked) {
                Some(screen) => Sync::Navigated(format!("Opened the {asked} screen."), screen),
                // Answered rather than dropped: the model reads this and picks
                // a real one on the next round.
                None => Sync::Answered(format!(
                    "error: no screen called {asked:?}. Pick one of: {}",
                    SCREENS.iter().map(|(n, _, _)| *n).collect::<Vec<_>>().join(", ")
                )),
            })
        }
        "api_write" => {
            let (method, path) = (string("method").to_ascii_uppercase(), string("path"));
            // Checked here as well as in `Client::api_write` so a malformed call
            // is answered in the transcript rather than becoming a confirm card
            // that can only fail once the user presses Run.
            if !path.starts_with("/api/v1/") || path.contains("..") {
                return Some(Sync::Answered(format!(
                    "error: path must start with /api/v1/ and may not contain '..', got {path:?}"
                )));
            }
            if !["POST", "PATCH", "PUT", "DELETE"].contains(&method.as_str()) {
                return Some(Sync::Answered(format!(
                    "error: method must be POST, PATCH, PUT or DELETE, got {method:?}"
                )));
            }
            Some(Sync::Parked(Box::new(Pending::Write {
                id: id.to_string(),
                method,
                path,
                body: args.get("body").cloned().unwrap_or_else(|| serde_json::json!({})),
            })))
        }
        "run_command" => {
            let command = string("command");
            // An unreadable call never becomes a card: the user would be
            // agreeing to a blank, and the model can be told to write it again.
            if command.trim().is_empty() {
                return Some(Sync::Answered(format!(
                    "error: run_command needs {{\"command\": \"…\"}}, got: {arguments}"
                )));
            }
            Some(Sync::Parked(Box::new(Pending::Command { id: id.to_string(), command })))
        }
        "web_search" => {
            let q = args.get("q").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
            let ask =
                args.get("ask").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
            // `q` wins when both are given — it is already an operator string,
            // so translating it too would be a round trip for nothing (same
            // precedence as the server's own `search_dork` route).
            let (query, verbatim) = match (q, ask) {
                (Some(q), _) => (q.to_string(), true),
                (None, Some(ask)) => (ask.to_string(), false),
                // Unreadable, same reasoning as `run_command`'s blank check: a
                // card with nothing to search is nothing the user can agree to.
                (None, None) => {
                    return Some(Sync::Answered(format!(
                        "error: web_search needs {{\"q\": \"…\"}} (a dork) or {{\"ask\": \"…\"}} \
                         (a sentence), got: {arguments}"
                    )));
                }
            };
            let raw_engine =
                args.get("engine").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
            let engine = match raw_engine {
                None => SearchEngine::Google,
                Some("google") => SearchEngine::Google,
                Some("duckduckgo") => SearchEngine::DuckDuckGo,
                Some("bing") => SearchEngine::Bing,
                // Named upfront rather than left to fail on Run: a bad engine
                // can only ever produce a card the user cannot usefully accept.
                Some(other) => {
                    return Some(Sync::Answered(format!(
                        "error: engine must be google, duckduckgo or bing, got {other:?}"
                    )));
                }
            };
            Some(Sync::Parked(Box::new(Pending::Search { id: id.to_string(), query, verbatim, engine })))
        }
        _ => None,
    }
}

/// Answer one `api_get` call. Errors come back as text — the model reads them
/// and corrects the path itself, which is why a 404 is not a failure here.
pub async fn run_api_get(client: &Client, arguments: &str) -> String {
    let path = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(str::to_string));
    let Some(path) = path else {
        return format!("error: api_get needs {{\"path\": \"/api/v1/…\"}}, got: {arguments}");
    };
    match client.api_get(&path).await {
        Ok(value) => value.to_string(),
        Err(e) => format!("error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_in_the_spec_resolves_to_a_real_one() {
        // The spec's enum and the lookup are the same table, so a screen the
        // model is offered can never be one `open_screen` then rejects.
        for (name, screen, _) in SCREENS {
            assert_eq!(screen_named(name), Some(screen), "{name}");
        }
        assert_eq!(screen_named("Teams"), Some(Screen::Teams), "case is not the user's problem");
        assert_eq!(screen_named("nowhere"), None);
    }

    #[test]
    fn a_bad_screen_is_answered_not_dropped() {
        let Sync::Answered(text) = run_sync_tool("1", "open_screen", r#"{"screen":"tacos"}"#).unwrap()
        else {
            panic!("a screen that does not exist must not navigate");
        };
        assert!(text.starts_with("error:"), "{text}");
        assert!(text.contains("teams"), "it must list the real ones: {text}");

        let out = run_sync_tool("1", "open_screen", r#"{"screen":"agenda"}"#).unwrap();
        assert!(matches!(out, Sync::Navigated(_, Screen::Agenda)), "{out:?}");

        assert!(run_sync_tool("1", "list_memories", "{}").is_none(), "not one of ours");
    }

    /// The confirm card is the only thing standing between a model and the
    /// user's data, so a write must never come back Answered.
    #[test]
    fn a_write_parks_and_never_runs_itself() {
        let out = run_sync_tool(
            "call_7",
            "api_write",
            r#"{"method":"post","path":"/api/v1/todos/boards/2/items","body":{"title":"ship it"}}"#,
        )
        .unwrap();
        let Sync::Parked(w) = out else { panic!("a write must park: {out:?}") };
        assert_eq!(w.id(), "call_7", "it has to answer the call that asked for it");
        assert_eq!(w.summary(), "POST /api/v1/todos/boards/2/items");
        let Pending::Write { method, body, .. } = &*w else { panic!() };
        assert_eq!(method, "POST", "lowercase from the model is still a POST");
        assert_eq!(body["title"], "ship it");
        assert!(w.detail().unwrap().contains("ship it"), "the body is on the card");
    }

    /// The shell is the biggest thing E.V. can reach, so it goes through the
    /// same gate — and a call with no command must never become a blank card.
    #[test]
    fn a_command_parks_and_a_blank_one_is_refused() {
        let out = run_sync_tool("c1", "run_command", r#"{"command":"git status"}"#).unwrap();
        let Sync::Parked(p) = out else { panic!("a command must park: {out:?}") };
        assert_eq!(p.summary(), "$ git status");
        assert_eq!(p.heading(), "Run this on your machine?");
        assert_eq!(p.detail(), None, "the command is the whole of it");

        for args in [r#"{"command":"   "}"#, "{}", "not json"] {
            match run_sync_tool("c1", "run_command", args).unwrap() {
                Sync::Answered(text) => assert!(text.starts_with("error:"), "{args}: {text}"),
                other => panic!("{args} must not become a card: {other:?}"),
            }
        }
    }

    /// Opening a browser leaves the app, so `web_search` goes through the same
    /// gate as `run_command` and must never answer itself.
    #[test]
    fn web_search_parks_and_never_answers_itself() {
        let out = run_sync_tool(
            "s1",
            "web_search",
            r#"{"q":"filetype:pdf intitle:\"Attention Is All You Need\""}"#,
        )
        .unwrap();
        let Sync::Parked(p) = out else { panic!("a search must park: {out:?}") };
        assert_eq!(p.id(), "s1", "it has to answer the call that asked for it");
        assert_eq!(
            p.summary(),
            "Search Google for: filetype:pdf intitle:\"Attention Is All You Need\""
        );
        assert_eq!(p.heading(), "Open this search in your browser?");
        assert_eq!(p.detail(), None, "the query is the whole of it");
        let Pending::Search { verbatim, engine, .. } = &*p else { panic!() };
        assert!(*verbatim, "q is a dork already written");
        assert_eq!(*engine, SearchEngine::Google, "default when engine is omitted");
    }

    /// `q` wins when both are given, matching the server's own precedence —
    /// translating a dork the model already wrote would be a round trip for
    /// nothing (`docs/web-search-module-plan.md`).
    #[test]
    fn a_dork_wins_over_a_sentence_when_both_are_given() {
        let out = run_sync_tool(
            "s2",
            "web_search",
            r#"{"q":"site:reddit.com keyboards","ask":"cheap keyboards","engine":"bing"}"#,
        )
        .unwrap();
        let Sync::Parked(p) = out else { panic!("{out:?}") };
        let Pending::Search { query, verbatim, engine, .. } = &*p else { panic!() };
        assert_eq!(query, "site:reddit.com keyboards");
        assert!(*verbatim);
        assert_eq!(*engine, SearchEngine::Bing);
    }

    /// A blank query and a bad engine are both answered before a card, not
    /// parked — a blank or unresolvable card is nothing the user can agree to.
    #[test]
    fn a_blank_query_and_a_bad_engine_are_refused_before_the_card() {
        for args in [r#"{"q":"   "}"#, r#"{"ask":""}"#, "{}", "not json"] {
            match run_sync_tool("s3", "web_search", args).unwrap() {
                Sync::Answered(text) => assert!(text.starts_with("error:"), "{args}: {text}"),
                other => panic!("{args} must not become a card: {other:?}"),
            }
        }
        match run_sync_tool("s3", "web_search", r#"{"ask":"cats","engine":"yahoo"}"#).unwrap() {
            Sync::Answered(text) => {
                assert!(text.starts_with("error:"), "{text}");
                assert!(text.contains("google"), "it must name the real ones: {text}");
            }
            other => panic!("a bad engine must not reach a confirm card: {other:?}"),
        }
    }

    #[test]
    fn a_write_outside_the_api_or_with_a_bad_method_is_refused_before_the_card() {
        for args in [
            r#"{"method":"POST","path":"/v1/chat/completions"}"#,
            r#"{"method":"POST","path":"/api/v1/../secrets"}"#,
            r#"{"method":"GET","path":"/api/v1/teams/"}"#,
            r#"{"method":"HEAD","path":"/api/v1/teams/"}"#,
        ] {
            match run_sync_tool("1", "api_write", args).unwrap() {
                Sync::Answered(text) => assert!(text.starts_with("error:"), "{args}: {text}"),
                other => panic!("{args} must not reach a confirm card: {other:?}"),
            }
        }
        // A body-less route still parks — {} is a legitimate body.
        let out = run_sync_tool("1", "api_write", r#"{"method":"POST","path":"/api/v1/assistant/items/4/complete"}"#).unwrap();
        let Sync::Parked(w) = out else { panic!("{out:?}") };
        assert_eq!(w.detail(), None, "an empty body is not worth a code block");
    }

    #[test]
    fn the_spec_covers_exactly_the_tools_that_are_dispatched() {
        let spec = tools_spec();
        let named: Vec<String> = spec
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(named, TOOLS, "a tool in the spec with no dispatch arm answers nothing");
    }
}
