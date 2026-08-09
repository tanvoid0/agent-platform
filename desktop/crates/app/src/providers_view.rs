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

    blocks.push(catalog_card(state));
    blocks.push(defaults_card(state));

    let page = ui::page(
        "Providers",
        Some(ui::muted("API keys, local endpoints and the model used when none is named.")),
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
