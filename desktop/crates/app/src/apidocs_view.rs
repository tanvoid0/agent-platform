//! Rendering for the API reference: how to connect, three quickstarts, and
//! every operation the server declares — each one a click away from a curl
//! command on the clipboard.

use crate::apidocs::{snippet, Endpoint, Message, Snippet, State};
use crate::ui::{self, space, Icon, Tone};
use iced::widget::container;
use iced::{Element, Length};

pub fn view<'a>(state: &'a State, origin: &str, key: &str) -> Element<'a, Message> {
    let blocks = vec![
        connect_card(origin, key),
        quickstart_card(state, origin),
        migration_card(),
        endpoints_card(state, origin, key),
    ];

    ui::page(
        "API",
        Some(ui::muted(
            "Everything this server exposes, straight from its own OpenAPI document — \
             for scripts, other apps, and anything else that talks HTTP.",
        )),
        Some(ui::button_secondary(Icon::Refresh, "Reload", Message::Refresh)),
        ui::stack_lg(blocks),
    )
}

/// Where to point a client and what to put in the header. The key is never
/// rendered — the copy buttons carry it, as on Settings → Status.
fn connect_card<'a>(origin: &str, key: &str) -> Element<'a, Message> {
    let mut actions = vec![
        ui::button_secondary(
            Icon::Copy,
            "Copy base URL",
            Message::Copy("origin".into(), origin.to_string()),
        ),
        ui::button_secondary(Icon::Copy, "Copy key", Message::Copy("key".into(), key.to_string())),
    ];
    if key.is_empty() {
        actions.push(ui::badge("no token — open on loopback", Tone::Success));
    }
    ui::card_with_header(
        "Connect",
        Some(ui::muted(
            "The local API is bound to loopback and every /api/v1 route needs a bearer. \
             Other apps on this machine send the key below — this server runs commands \
             and holds your provider keys, so it is not open the way an inference \
             endpoint is.",
        )),
        None,
        ui::stack(vec![
            ui::field("Base URL", ui::mono(origin.to_string())),
            ui::field(
                "Auth header",
                ui::mono(
                    if key.is_empty() {
                        "(none on loopback)".to_string()
                    } else {
                        "Authorization: Bearer <key>".to_string()
                    },
                ),
            ),
            ui::field(
                "Keys",
                ui::muted(
                    "The master key (Settings → Status) has full access. Scoped, \
                     rate-limited workspace tokens start with `agp_` and are minted \
                     through POST /api/v1/workspaces/{workspace_id}/api-tokens \
                     - use those for anything outside this machine.",
                ),
            ),
            ui::field(
                "Errors",
                ui::mono(
                    "{\"error\": {\"message\", \"type\", \"code\", \"request_id\"}}".to_string(),
                ),
            ),
            ui::field(
                "Correlation",
                ui::muted(
                    "Every response carries X-Request-ID. Send your own to have it kept, \
                     and the same id shows up in Settings → Logs.",
                ),
            ),
            ui::cluster(actions).into(),
        ]),
    )
}

/// Three runnable starting points, one visible at a time.
fn quickstart_card<'a>(state: &'a State, origin: &str) -> Element<'a, Message> {
    let text = snippet(state.snippet, origin);
    let tabs = ui::segmented(
        Snippet::ALL.map(|s| (s.label(), s == state.snippet, Message::SnippetChanged(s))),
    );
    ui::card_with_header(
        "Quickstart",
        Some(ui::muted("Copy, replace the key, run.")),
        Some(copy_button(state, "quickstart", "Copy", text.clone())),
        ui::stack(vec![tabs, ui::code(ui::mono(text))]),
    )
}

/// What the two halves of the server mean for a client: nothing, which is the
/// point worth saying out loud.
fn migration_card<'a>() -> Element<'a, Message> {
    ui::card_with_header(
        "Rust and Python",
        Some(ui::muted(
            "The API is being moved from the Python server to agent-platformd (Rust) one \
             domain at a time. Both live behind the same port.",
        )),
        None,
        ui::stack(vec![
            ui::body(
                "A migrated route answers exactly what it answered before — same paths, same \
                 bodies, same status codes, same error envelope. Clients written against the \
                 old server keep working, and nothing below needs a different call depending \
                 on who serves it.",
            ),
            ui::cluster(vec![
                ui::badge("rust", Tone::Info),
                ui::muted("answered by agent-platformd: projects, teams, todo boards, workflows and the whole /v1 proxy"),
            ])
            .into(),
            ui::cluster(vec![
                ui::badge("python", Tone::Neutral),
                ui::muted("proxied byte-for-byte to the Python server: processes, assistant, coder, chat, model ops, workspaces, tokens"),
            ])
            .into(),
        ]),
    )
}

fn endpoints_card<'a>(state: &'a State, origin: &str, key: &str) -> Element<'a, Message> {
    let visible = state.visible();
    let body: Element<'a, Message> = if let Some(error) = &state.error {
        ui::stack(vec![
            ui::alert_error_traced(&format!("Could not read /openapi.json — {error}"), Message::TraceLogs),
            ui::muted(
                "The list comes from the running server. Start it on Settings → Status and \
                 reload; the quickstart above works without it.",
            ),
        ])
        .into()
    } else if !state.loaded {
        ui::empty_state_icon(Icon::Clock, "Reading the server's OpenAPI document…")
    } else if visible.is_empty() {
        ui::empty_state_icon(Icon::Search, "No endpoint matches that filter.")
    } else {
        let mut rows: Vec<Element<'a, Message>> = Vec::new();
        let mut tag: Option<&str> = None;
        for endpoint in visible.iter().copied() {
            if tag != Some(endpoint.tag.as_str()) {
                tag = Some(endpoint.tag.as_str());
                rows.push(group_header(state, endpoint.tag.as_str()));
            }
            rows.push(row(state, endpoint, origin, key));
        }
        ui::stack(rows).into()
    };

    ui::card_with_header(
        "Endpoints",
        Some(ui::muted(format!(
            "{} across {}. Click one for its parameters, its body and a curl command.",
            ui::count(state.endpoints.len(), "operation", "operations"),
            ui::count(tag_count(state), "group", "groups"),
        ))),
        None,
        ui::stack(vec![
            container(ui::input_icon(
                Icon::Search,
                "Filter by path, summary or method…",
                &state.filter,
                Message::FilterChanged,
            ))
            .width(360)
            .into(),
            body,
        ]),
    )
}

fn tag_count(state: &State) -> usize {
    let mut tags: Vec<&str> = state.endpoints.iter().map(|e| e.tag.as_str()).collect();
    tags.dedup();
    tags.len()
}

fn group_header<'a>(state: &'a State, tag: &str) -> Element<'a, Message> {
    let n = state.endpoints.iter().filter(|e| e.tag == tag && e.matches(&state.filter)).count();
    ui::cluster(vec![
        ui::heading(tag.to_string()),
        ui::badge(ui::count(n, "route", "routes"), Tone::Neutral),
    ])
    .into()
}

/// One operation: the clickable summary line, and its detail when open.
fn row<'a>(state: &'a State, endpoint: &Endpoint, origin: &str, key: &str) -> Element<'a, Message> {
    let id = endpoint.id();
    let open = state.open.as_deref() == Some(id.as_str());

    let mut line = vec![
        container(ui::badge(endpoint.method.clone(), method_tone(&endpoint.method)))
            .width(84)
            .into(),
        container(ui::mono(endpoint.path.clone())).width(Length::Fill).into(),
    ];
    if !endpoint.auth {
        line.push(ui::badge("no key", Tone::Success));
    }

    let head = ui::list_item(ui::cluster(line), open, Message::Toggle(id.clone()));
    if !open {
        return head;
    }
    ui::stack(vec![head, detail(state, endpoint, origin, key)]).into()
}

fn detail<'a>(
    state: &'a State,
    endpoint: &Endpoint,
    origin: &str,
    key: &str,
) -> Element<'a, Message> {
    let id = endpoint.id();
    let curl = endpoint.curl(origin, key);
    let mut parts: Vec<Element<'a, Message>> = Vec::new();

    if !endpoint.summary.is_empty() {
        parts.push(ui::body(endpoint.summary.clone()));
    }
    if !endpoint.description.is_empty() {
        parts.push(ui::muted(endpoint.description.clone()));
    }
    if !endpoint.params.is_empty() {
        parts.push(ui::caption("Parameters"));
        for p in &endpoint.params {
            parts.push(
                ui::cluster(vec![
                    container(ui::mono(p.name.clone())).width(200).into(),
                    container(ui::caption(p.location.clone())).width(64).into(),
                    container(ui::caption(p.kind.clone())).width(Length::Fill).into(),
                    if p.required {
                        ui::badge("required", Tone::Warning)
                    } else {
                        ui::caption("optional")
                    },
                ])
                .into(),
            );
        }
    }
    if let Some(body) = &endpoint.body {
        parts.push(ui::caption("Request body"));
        parts.push(ui::code(ui::mono(body.clone())));
    }
    parts.push(ui::caption("curl"));
    parts.push(ui::code(ui::mono(curl.clone())));

    let mut buttons = vec![
        copy_button(state, &format!("{id}:curl"), "Copy curl", curl),
        copy_button(state, &format!("{id}:url"), "Copy URL", format!("{origin}{}", endpoint.path)),
    ];
    if let Some(body) = &endpoint.body {
        buttons.push(copy_button(state, &format!("{id}:body"), "Copy body", body.clone()));
    }
    parts.push(ui::cluster(buttons).into());

    container(ui::stack(parts))
        .padding(iced::Padding::default().left(space::MD).bottom(space::SM))
        .into()
}

/// A copy button that says it worked. The id keeps two buttons on the same row
/// from both claiming a copy that only one of them made.
fn copy_button<'a>(
    state: &State,
    id: &str,
    label: &'static str,
    text: String,
) -> Element<'a, Message> {
    let done = state.copied.as_deref() == Some(id);
    ui::button_secondary(
        Icon::Copy,
        if done { "Copied!" } else { label },
        Message::Copy(id.to_string(), text),
    )
}

/// Read-then-write, in the order a reader scans: safe, creating, changing,
/// destroying.
fn method_tone(method: &str) -> Tone {
    match method {
        "GET" => Tone::Info,
        "POST" => Tone::Success,
        "PUT" | "PATCH" => Tone::Warning,
        "DELETE" => Tone::Danger,
        _ => Tone::Neutral,
    }
}
