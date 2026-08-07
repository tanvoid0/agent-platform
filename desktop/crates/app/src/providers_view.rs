//! Provider settings rendering: catalog status, BYOK keys, local endpoints and
//! the default provider/model pair.

use crate::providers::{Message, State, ENDPOINT_FIELDS, SECRET_FIELDS};
use crate::ui::{self, Icon, Tone};
use iced::widget::container;
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(dismissible(ui::alert_error_traced(err, Message::TraceLogs)));
    }

    blocks.push(catalog_card(state));
    blocks.push(defaults_card(state));
    blocks.push(keys_card(state));

    ui::page(
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
    )
}

fn dismissible(inner: Element<'_, Message>) -> Element<'_, Message> {
    ui::cluster(vec![
        container(inner).width(Length::Fill).into(),
        ui::button_ghost(Icon::X, "Dismiss", Message::Dismiss),
    ])
    .into()
}

fn catalog_card(state: &State) -> Element<'_, Message> {
    let list: Element<'_, Message> = if state.catalog.is_empty() {
        if state.catalog_loaded {
            ui::empty_state("No provider catalog yet.")
        } else {
            ui::empty_state_icon(Icon::Clock, "Loading catalog…")
        }
    } else {
        ui::stack(
            state
                .catalog
                .iter()
                .map(|p| {
                    let (label, tone) = if p.configured {
                        ("configured", Tone::Success)
                    } else {
                        ("not configured", Tone::Neutral)
                    };
                    let mut cells = vec![
                        // Fixed name column: with the name filling, each row
                        // split its slack differently and the badges stepped
                        // left and right down the list.
                        container(ui::body(p.label.clone())).width(180).into(),
                        ui::badge(label, tone),
                        ui::spacer(),
                    ];
                    if p.local {
                        cells.push(ui::badge("local", Tone::Info));
                    }
                    cells.push(ui::caption(format!(
                        "{} · {}",
                        ui::count(p.models.options.len(), "model", "models"),
                        p.models.source
                    )));
                    let mut rows = vec![ui::cluster(cells).into()];
                    // The catalog degrades quietly to aliases or hard-coded
                    // fallbacks, which looks like success unless it is said.
                    if let Some(note) = p.models.fallback_note.as_ref().or(p.models.warning.as_ref()) {
                        rows.push(ui::caption(note.clone()));
                    }
                    ui::stack(rows).into()
                })
                .collect::<Vec<_>>(),
        )
        .into()
    };

    ui::card_with_header(
        "Catalog",
        Some(ui::muted("What the proxy can reach right now.")),
        None,
        list,
    )
}

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

fn keys_card(state: &State) -> Element<'_, Message> {
    let mut rows: Vec<Element<'_, Message>> = Vec::new();

    for (key, label) in SECRET_FIELDS {
        let stored = state.env_key(key);
        let is_set = stored.is_some_and(|k| k.set);
        let placeholder = if is_set { "stored — type to replace" } else { "not set" };
        let mut cells = vec![container(ui::input(
            placeholder,
            state.draft(key),
            move |v| Message::FieldChanged(key, v),
        ))
        .width(Length::Fill)
        .into()];
        if is_set && !state.edited(key) {
            cells.push(ui::mono(stored.map(|k| k.masked.as_str()).unwrap_or_default()));
        }
        rows.push(ui::field(label, ui::cluster(cells)));
    }

    rows.push(ui::separator());

    // Non-secret values come back in full, so an untouched endpoint field shows
    // what is stored rather than an empty box.
    for (key, label, placeholder) in ENDPOINT_FIELDS {
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

    ui::card_with_header(
        "Keys and endpoints",
        Some(ui::muted("Saved to the proxy's .env. Keys are write-only: what is stored is never shown.")),
        None,
        ui::stack(rows),
    )
}
