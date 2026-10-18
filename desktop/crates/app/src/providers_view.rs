//! Provider settings rendering: the catalog is the screen, and each row opens
//! that provider's own keys/endpoint/model dialog. One flat list of `.env`
//! fields made the user match "AI/ML API key" to the "AIMLAPI" row by hand.

use crate::providers::{meta, Message, ProviderMeta, State};
use crate::ui::{self, Icon, Tone};
use agent_platform_client::types::ProviderEntry;
use iced::widget::{container, scrollable};
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::Dismiss, Vec::new()));
    }

    // Grouped by what a provider *produces*, not by who sells it. A local
    // install needs three different programs to cover what one cloud vendor
    // used to, and "which of these writes and which of these draws" is the
    // question the old flat catalog could not answer.
    blocks.push(routing_card(state));
    blocks.push(ui::heading("Text"));
    blocks.push(catalog_card(state));
    blocks.push(defaults_card(state));
    blocks.push(ui::heading("Images & video"));
    blocks.push(media_card(state));
    blocks.push(ui::heading("Speech"));
    blocks.push(speech_card(state));
    blocks.push(ui::heading("Other"));
    blocks.push(search_card(state));

    let page = ui::page(
        "Models",
        Some(ui::muted(
            "Who writes, who draws, who speaks — and the keys and endpoints behind each.",
        )),
        Some(
            ui::cluster(vec![
                if state.busy {
                    ui::badge("saving…", Tone::Info)
                } else {
                    ui::button_default(Icon::Save, "Save", Message::Save)
                },
                ui::button_outline(Icon::Refresh, "Refresh", Message::Refresh),
            ])
            .into(),
        ),
        ui::stack_lg(blocks),
    );

    match state.open.as_deref().and_then(|id| state.entry(id)) {
        Some(entry) => ui::modal(page, provider_modal(state, entry), 560.0),
        None => page,
    }
}

// ---------------------------------------------------------------------------
// What runs what
// ---------------------------------------------------------------------------

/// One line per modality, naming the provider a request would actually reach.
///
/// The server resolves this per capability (`llm_config`'s router, ADR 0018),
/// and until this card existed the answer was only visible by making a request
/// and seeing what came back. A user with Ollama for text and ComfyUI for
/// pictures should be able to read that off one line rather than infer it from
/// three cards.
fn routing_card(state: &State) -> Element<'_, Message> {
    let text = match state.env.as_ref().map(|e| &e.resolved_defaults) {
        Some(d) if !d.provider.is_empty() => {
            let label = state
                .entry(&d.provider)
                .map(|e| e.label.clone())
                .unwrap_or_else(|| d.provider.clone());
            if d.model.is_empty() { label } else { format!("{label} · {}", d.model) }
        }
        _ => "not configured".to_string(),
    };

    let pictures = match state.media.as_ref() {
        Some(status) if status.reachable => {
            let name = media_label(&status.backend);
            match status.image_model.as_deref() {
                Some(model) => format!("{name} · {model}"),
                None => format!("{name} · no checkpoint installed"),
            }
        }
        Some(status) => format!("{} · not reachable", media_label(&status.backend)),
        None => "checking…".to_string(),
    };

    let speech = if state.env_set("SPEECH_API_BASE") { "configured" } else { "not configured" };

    ui::section(
        "What runs what",
        None,
        ui::stack(vec![
            ui::field("Text", ui::body(text)),
            ui::field("Images & video", ui::body(pictures)),
            ui::field("Speech", ui::body(speech.to_string())),
        ]),
    )
}

/// A local backend that answered with a model list is running. `build_admin`
/// empties the options of a local provider it could not reach, so this is the
/// same fact its warning line states, in a form a badge can show.
fn running(entry: &ProviderEntry) -> bool {
    entry.local && !entry.models.options.is_empty()
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

fn catalog_card(state: &State) -> Element<'_, Message> {
    let list: Element<'_, Message> = if state.catalog.is_empty() {
        if state.catalog_loaded {
            ui::empty_state("No provider catalog yet.")
        } else {
            ui::empty_state_icon(Icon::Clock, "Loading catalog…")
        }
    } else {
        ui::stack(state.catalog.iter().map(catalog_row).collect::<Vec<_>>()).into()
    };

    ui::card_with_header(
        "Catalog",
        Some(ui::muted("What the proxy can reach right now. Open one to set its key or model.")),
        None,
        list,
    )
}

fn catalog_row(entry: &ProviderEntry) -> Element<'_, Message> {
    let m = meta(&entry.id);
    let (glyph, label, tone) = if entry.configured {
        (Icon::CheckCircle, "configured", Tone::Success)
    } else {
        (Icon::XCircle, "not configured", Tone::Danger)
    };

    // Fixed name column: with the name filling, each row split its slack
    // differently and the badges stepped left and right down the list.
    let mut cells = vec![
        container(ui::body(entry.label.clone())).width(180).into(),
        ui::badge_icon(glyph, label, tone),
    ];
    if entry.local {
        cells.push(if running(entry) {
            ui::badge_icon(Icon::Check, "running", Tone::Success)
        } else {
            ui::badge_icon(Icon::X, "stopped", Tone::Warning)
        });
    }
    cells.push(ui::spacer());
    // Fixed columns for the two trailing slots: only some rows have an action,
    // and letting the row split its own slack stepped the model counts left and
    // right down the list.
    cells.push(
        container(ui::caption(format!(
            "{} · {}",
            ui::count(entry.models.options.len(), "model", "models"),
            entry.models.source
        )))
        .width(220)
        .align_x(iced::Alignment::End)
        .into(),
    );
    // The one action this row is actually missing, inline: start a local
    // backend that is down, or go mint the key a cloud one has not got.
    cells.push(
        container(row_action(entry, m)).width(150).align_x(iced::Alignment::End).into(),
    );
    cells.push(ui::button_outline(Icon::Settings, "Configure", Message::Open(entry.id.clone())));

    let mut rows = vec![ui::cluster(cells).into()];
    // The catalog degrades quietly to aliases or hard-coded fallbacks, which
    // looks like success unless it is said.
    if let Some(note) = entry.models.fallback_note.as_ref().or(entry.models.warning.as_ref()) {
        rows.push(ui::caption(note.clone()));
    }
    ui::stack(rows).into()
}

/// `Launch` for a stopped local backend, `Get API key` for an unconfigured
/// cloud one, and nothing when neither applies.
fn row_action<'a>(
    entry: &ProviderEntry,
    m: Option<&'static ProviderMeta>,
) -> Element<'a, Message> {
    match m {
        Some(m) if entry.local && !running(entry) && m.launch.is_some() => {
            ui::button_secondary(Icon::Play, "Launch", Message::Launch(m.id))
        }
        Some(ProviderMeta { key_url: Some(url), .. }) if !entry.configured => {
            ui::button_secondary(Icon::Globe, "Get API key", Message::OpenUrl(url))
        }
        _ => container(ui::body("")).into(),
    }
}

// ---------------------------------------------------------------------------
// Web search (ADR 0008's amendment) — not a catalog row, see providers.rs's
// module doc comment for why.
// ---------------------------------------------------------------------------

/// Google requires two separate things: an API key from the Cloud console,
/// and a Programmable Search Engine id (`cx`) from its own control panel. A
/// user who only gets the key is stuck with no way to know a second signup
/// is needed, so both links are offered up front rather than one.
const SEARCH_KEY_URL: &str = "https://console.cloud.google.com/apis/credentials";
const SEARCH_ENGINE_URL: &str = "https://programmablesearchengine.google.com/controlpanel/create";

fn search_card(state: &State) -> Element<'_, Message> {
    let (glyph, label, tone) = if state.search_configured() {
        (Icon::CheckCircle, "configured", Tone::Success)
    } else {
        (Icon::XCircle, "not configured", Tone::Danger)
    };

    let mut rows: Vec<Element<'_, Message>> = vec![
        ui::cluster(vec![ui::badge_icon(glyph, label, tone), ui::spacer()]).into(),
        ui::muted(
            "Google Programmable Search — free for up to 100 queries/day. Without it, the \
             Search screen hands your query off to your browser instead, which is not a \
             failure state; it's what the module does out of the box.",
        ),
    ];

    // Both fields are required together (`SearchBackend::from_env`): a key
    // with no cx is unconfigured, not half-configured, so say which one is
    // still missing rather than letting a one-field save look like it did
    // nothing.
    if let Some(missing) = state.search_missing() {
        rows.push(ui::caption(format!("Not configured — {missing} is still missing.")));
    }

    let key_set = state.env_key("SEARCH_API_KEY").is_some_and(|k| k.set);
    let key_placeholder = if key_set { "stored — type to replace" } else { "not set" };
    let mut key_cells = vec![container(ui::input(
        key_placeholder,
        state.draft("SEARCH_API_KEY"),
        |v| Message::FieldChanged("SEARCH_API_KEY", v),
    ))
    .width(Length::Fill)
    .into()];
    if key_set && !state.edited("SEARCH_API_KEY") {
        key_cells.push(ui::mono(
            state.env_key("SEARCH_API_KEY").map(|k| k.masked.as_str()).unwrap_or_default(),
        ));
    }
    rows.push(ui::field("API key", ui::cluster(key_cells)));

    // Not a secret (`SENSITIVE_ENV_KEYS` excludes `SEARCH_CX` — it names an
    // engine, not an account), so it comes back in full and shows what is
    // stored the same way a base URL field does.
    let cx_value = if state.edited("SEARCH_CX") {
        state.draft("SEARCH_CX")
    } else {
        state.env_key("SEARCH_CX").map(|k| k.value.as_str()).unwrap_or_default()
    };
    rows.push(ui::field(
        "Search engine ID (cx)",
        ui::input("not set", cx_value, |v| Message::FieldChanged("SEARCH_CX", v)),
    ));

    rows.push(ui::cluster(vec![
        ui::button_ghost(Icon::Globe, "Get an API key", Message::OpenUrl(SEARCH_KEY_URL)),
        // A fresh engine defaults to a fixed site list; it must be switched to
        // search the entire web, or every query it runs comes back empty.
        ui::button_ghost(Icon::Globe, "Create a search engine", Message::OpenUrl(SEARCH_ENGINE_URL)),
    ])
    .into());

    ui::card_with_header(
        "Web search",
        Some(ui::muted("Lets the Search screen (and E.V.) read results back, not just build the query.")),
        None,
        ui::stack(rows),
    )
}

// ---------------------------------------------------------------------------
// Media backend (ADR 0009 / 0011) — a provider for images and video, not chat,
// so it is a card of its own for the same reason web search is: no key, no
// model dropdown, no `ProviderMeta` row to hang it on.
// ---------------------------------------------------------------------------

const COMFY_URL: &str = "https://www.comfy.org/download";

/// The two media backends by name. `MEDIA_BACKEND` names them `comfy` and
/// `sdcpp` on the wire; nobody calls them that out loud.
fn media_label(backend: &str) -> &'static str {
    match backend {
        "sdcpp" => "stable-diffusion.cpp",
        _ => "ComfyUI",
    }
}

fn media_card(state: &State) -> Element<'_, Message> {
    let status = state.media.as_ref();
    let reachable = status.is_some_and(|s| s.reachable);
    let name = media_label(status.map(|s| s.backend.as_str()).unwrap_or_default());

    let mut head = vec![
        container(ui::body(name.to_string())).width(180).into(),
        if reachable {
            ui::badge_icon(Icon::Check, "running", Tone::Success)
        } else {
            ui::badge_icon(Icon::X, "not reachable", Tone::Warning)
        },
        ui::spacer(),
    ];
    if reachable {
        if let Some(s) = status {
            // What it can actually be asked for right now, which is the half
            // "running" does not answer: sd-server binds one model at startup.
            let modes = if s.modes.is_empty() {
                "image, video".to_string()
            } else {
                s.modes
                    .iter()
                    .map(|m| if m == "vid_gen" { "video" } else { "image" })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            head.push(ui::caption(format!(
                "{modes} · {}",
                ui::count(s.checkpoints.len(), "checkpoint", "checkpoints")
            )));
        }
    } else {
        head.push(ui::button_secondary(Icon::Globe, "Install ComfyUI", Message::OpenUrl(COMFY_URL)));
    }
    head.push(ui::button_outline(Icon::Download, "Model files", Message::OpenDownloads));

    let mut rows: Vec<Element<'_, Message>> = vec![
        ui::cluster(head).into(),
        ui::muted(
            "Image and video generation for the Studio screen. Not an LLM provider — no key,              no model default; it is reachable or it is not.",
        ),
    ];

    // sd.cpp is managed by the server, so its stage says more than "not
    // reachable" does — downloading and not-installed are both unreachable.
    if let Some(detail) = status.and_then(|s| s.backend_detail.clone()) {
        rows.push(ui::caption(detail));
    }
    // The per-modality default for images: pick one of the installed
    // checkpoints, or leave it unset and let the server prefer a known
    // text-to-image family. Video has no twin — its template names its own
    // model family, so there is nothing to choose.
    let checkpoints = status.map(|s| s.checkpoints.clone()).unwrap_or_default();
    if !checkpoints.is_empty() {
        let chosen = if state.edited("MEDIA_IMAGE_MODEL") {
            state.draft("MEDIA_IMAGE_MODEL").to_string()
        } else {
            state.env_key("MEDIA_IMAGE_MODEL").map(|k| k.value.clone()).unwrap_or_default()
        };
        rows.push(ui::field(
            "Image model",
            ui::select("Auto — first known family", checkpoints, Some(chosen).filter(|c| !c.is_empty()), |v| {
                Message::FieldChanged("MEDIA_IMAGE_MODEL", v)
            }),
        ));
    }
    if let Some(model) = status.and_then(|s| s.image_model.clone()) {
        rows.push(ui::caption(format!("Rendering with {model}.")));
    }

    let base = if state.edited("MEDIA_API_BASE") {
        state.draft("MEDIA_API_BASE")
    } else {
        state.env_key("MEDIA_API_BASE").map(|k| k.value.as_str()).unwrap_or_default()
    };
    // The server always resolves a base, so the placeholder is what it is
    // using right now rather than a guess — leaving the field empty keeps it.
    let placeholder =
        status.map(|s| s.base.as_str()).filter(|b| !b.is_empty()).unwrap_or(DEFAULT_MEDIA_BASE);
    rows.push(ui::field(
        "Base URL",
        ui::input(placeholder, base, |v| Message::FieldChanged("MEDIA_API_BASE", v)),
    ));

    ui::card_with_header(
        "Media backend",
        Some(ui::muted("Where Studio sends image and video jobs.")),
        None,
        ui::stack(rows),
    )
}

const DEFAULT_MEDIA_BASE: &str = "http://127.0.0.1:8188";

// ---------------------------------------------------------------------------
// Speech — `Modality::Speech` has been in the capability router since the port
// and had nowhere to be set. Two fields, no models list, so it is a card of its
// own for the same reason web search is.
// ---------------------------------------------------------------------------

fn speech_card(state: &State) -> Element<'_, Message> {
    let configured = state.env_set("SPEECH_API_BASE");
    let (glyph, label, tone) = if configured {
        (Icon::CheckCircle, "configured", Tone::Success)
    } else {
        (Icon::XCircle, "not configured", Tone::Danger)
    };

    let base = if state.edited("SPEECH_API_BASE") {
        state.draft("SPEECH_API_BASE")
    } else {
        state.env_key("SPEECH_API_BASE").map(|k| k.value.as_str()).unwrap_or_default()
    };

    let key_set = state.env_key("SPEECH_API_KEY").is_some_and(|k| k.set);
    let key_placeholder = if key_set { "stored — type to replace" } else { "not set (often unused)" };
    let mut key_cells = vec![container(ui::input(
        key_placeholder,
        state.draft("SPEECH_API_KEY"),
        |v| Message::FieldChanged("SPEECH_API_KEY", v),
    ))
    .width(Length::Fill)
    .into()];
    if key_set && !state.edited("SPEECH_API_KEY") {
        key_cells.push(ui::mono(
            state.env_key("SPEECH_API_KEY").map(|k| k.masked.as_str()).unwrap_or_default(),
        ));
    }

    ui::card_with_header(
        "Text to speech",
        Some(ui::muted(
            "An OpenAI-shaped /v1/audio/speech backend. Unset, the proxy answers 501 for speech              and the app falls back to the system voice — not a failure state.",
        )),
        None,
        ui::stack(vec![
            ui::cluster(vec![ui::badge_icon(glyph, label, tone), ui::spacer()]).into(),
            ui::field(
                "Base URL",
                ui::input("not set", base, |v| Message::FieldChanged("SPEECH_API_BASE", v)),
            ),
            ui::field("API key", ui::cluster(key_cells)),
        ]),
    )
}

// ---------------------------------------------------------------------------
// Per-provider dialog
// ---------------------------------------------------------------------------

fn provider_modal<'a>(state: &'a State, entry: &'a ProviderEntry) -> Element<'a, Message> {
    let m = meta(&entry.id);
    let mut rows: Vec<Element<'a, Message>> = Vec::new();

    if let Some(note) = entry.models.warning.as_ref().or(entry.models.fallback_note.as_ref()) {
        rows.push(ui::alert(Tone::Warning, note.clone(), None));
    }

    if let Some((key, label)) = m.and_then(|m| m.secret) {
        let stored = state.env_key(key);
        let is_set = stored.is_some_and(|k| k.set);
        let placeholder = if is_set { "stored — type to replace" } else { "not set" };
        let mut cells = vec![container(ui::input(placeholder, state.draft(key), move |v| {
            Message::FieldChanged(key, v)
        }))
        .width(Length::Fill)
        .into()];
        if is_set && !state.edited(key) {
            cells.push(ui::mono(stored.map(|k| k.masked.as_str()).unwrap_or_default()));
        }
        rows.push(ui::field(label, ui::cluster(cells)));
    }

    // Non-secret values come back in full, so an untouched endpoint field shows
    // what is stored rather than an empty box.
    if let Some((key, label, placeholder)) = m.and_then(|m| m.endpoint) {
        let value = if state.edited(key) {
            state.draft(key)
        } else {
            state.env_key(key).map(|k| k.value.as_str()).unwrap_or_default()
        };
        rows.push(ui::field(
            label,
            ui::input(placeholder, value, move |v| Message::FieldChanged(key, v)),
        ));
    }

    // An unconfigured or unreachable provider has no models, and an empty
    // dropdown is a control that cannot be used — say why instead.
    let options = state.models_of(&entry.id);
    if options.is_empty() {
        rows.push(ui::caption(
            "No models to choose from yet. Set a key (or start the backend), then Refresh.",
        ));
    } else {
        let id = entry.id.clone();
        let selected = (state.default_provider == entry.id && !state.default_model.is_empty())
            .then(|| state.default_model.clone());
        rows.push(ui::field(
            "Default model",
            ui::select("Pick a model", options, selected, move |model| {
                Message::ProviderModelPicked(id.clone(), model)
            }),
        ));
        rows.push(ui::caption("Picking a model here also makes this the default provider."));
    }

    // Model ops' "Local models" card, moved to where a model is actually
    // chosen: the list of what is on this machine, and the field that adds to
    // it. Only Ollama can pull, so only Ollama grows this half.
    if m.is_some_and(|m| m.pullable) {
        rows.push(ui::separator());
        rows.push(ui::field(
            "Download a model",
            ui::cluster(vec![
                container(ui::input("qwen2.5:7b", &state.pull_name, Message::PullNameChanged))
                    .width(Length::Fill)
                    .into(),
                if state.busy {
                    ui::badge("pulling…", Tone::Info)
                } else {
                    ui::button_secondary(Icon::Download, "Pull", Message::PullModel)
                },
            ]),
        ));
        rows.push(if state.local_models.is_empty() {
            ui::empty_state_icon(Icon::Cpu, "No local models found (is Ollama running?).")
        } else {
            // Bounded, and scrolling inside itself: a dozen models would
            // otherwise push the dialog's own buttons off the window.
            scrollable(ui::stack(
                state
                    .local_models
                    .iter()
                    .map(|model| {
                        ui::cluster(vec![
                            ui::mono(model.name.clone()),
                            ui::spacer(),
                            ui::caption(
                                model.size.map(crate::domain::format_size).unwrap_or_default(),
                            ),
                        ])
                        .into()
                    })
                    .collect::<Vec<_>>(),
            ))
            .spacing(ui::space::SM)
            .height(180)
            .into()
        });
    }

    let mut actions = vec![ui::spacer()];
    if let Some(url) = m.and_then(|m| m.key_url) {
        actions.push(ui::button_ghost(Icon::Globe, "Get API key", Message::OpenUrl(url)));
    }
    actions.push(ui::button_outline(Icon::X, "Close", Message::Close));
    actions.push(ui::button_default(Icon::Save, "Save", Message::Save));
    rows.push(ui::cluster(actions).into());

    ui::card(
        ui::stack(vec![
            ui::heading(entry.label.clone()),
            ui::muted("Saved to the proxy's .env. Keys are write-only: what is stored is never shown."),
            ui::separator(),
            ui::stack(rows).into(),
        ])
        .spacing(crate::ui::space::MD),
    )
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn defaults_card(state: &State) -> Element<'_, Message> {
    let mut rows = vec![
        ui::field(
            "Provider",
            ui::select(
                "Pick a provider",
                state.provider_ids(),
                (!state.default_provider.is_empty()).then(|| state.default_provider.clone()),
                Message::DefaultProviderChanged,
            ),
        ),
        ui::field(
            "Model",
            ui::select(
                "Pick a model",
                state.model_options(),
                (!state.default_model.is_empty()).then(|| state.default_model.clone()),
                Message::DefaultModelChanged,
            ),
        ),
    ];

    // Persisted and resolved diverge whenever the saved provider is unusable —
    // the request still succeeds, against a different provider than the one shown.
    if let Some(env) = &state.env {
        let resolved = &env.resolved_defaults;
        let persisted = &env.persisted_defaults;
        if !persisted.provider.is_empty() && resolved.provider != persisted.provider {
            rows.push(ui::alert(
                Tone::Warning,
                format!(
                    "Requests are going to “{}” instead: the saved provider is not configured.",
                    resolved.provider
                ),
                None,
            ));
        }
    }

    ui::card_with_header(
        "Defaults",
        Some(ui::muted("Used when a request does not name a model.")),
        None,
        ui::stack(rows),
    )
}
