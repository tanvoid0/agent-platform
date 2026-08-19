//! Web search rendering (ADR 0008, `docs/web-search-module-plan.md`): the
//! sentence box, the editable dork it translates to, removable chips and an
//! add-operator row for each part, the explanation, the results (once a key
//! is configured) or the same unconfigured view the module has always shown,
//! and search history. State and `update` live in `search.rs`, per the root
//! `CLAUDE.md` split.

use crate::search::{AddField, Message, Mode, State};
use crate::ui::{self, Icon, Tone};
use agent_platform_client::types::{DorkChip, DorkExplanationLine, SearchHistoryEntry, SearchResult, SEARCH_ENGINES};
use iced::widget::container;
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::Dismiss, Vec::new()));
    }

    blocks.push(ask_card(state));
    blocks.push(query_card(state));

    if let Some(response) = &state.response {
        if !response.dork.explanation.is_empty() {
            blocks.push(ui::card_with_header(
                "What this means",
                None,
                None,
                explanation_view(&response.dork.explanation),
            ));
        }
        // `configured` — not an empty `results` list — is what decides which
        // of these renders. See `search.rs`'s module doc comment and the ADR
        // 0008 amendment: an unconfigured install must look like the whole
        // product, not a degraded one.
        if response.configured {
            blocks.push(results_card(response.results.as_slice(), response.total_estimate));
        } else if !state.hint_dismissed {
            blocks.push(unconfigured_hint());
        }
    }

    blocks.push(history_card(state));

    ui::page(
        "Search",
        Some(ui::muted(
            "Describe what you want. We turn it into a precise query you can edit, \
             then open it in the browser.",
        )),
        None,
        ui::stack_lg(blocks),
    )
}

fn ask_card(state: &State) -> Element<'_, Message> {
    ui::card_with_header(
        "Ask",
        Some(ui::muted("Describe what you're looking for, in plain language.")),
        None,
        ui::cluster(vec![
            container(ui::input_submit(
                r#"e.g. find a pdf with this title "Attention Is All You Need""#,
                &state.ask,
                Message::AskChanged,
                Message::Run,
            ))
            .width(Length::Fill)
            .into(),
            if state.busy {
                ui::badge("searching…", Tone::Info)
            } else {
                ui::button_default(Icon::Search, "Search", Message::Run)
            },
        ]),
    )
}

fn query_card(state: &State) -> Element<'_, Message> {
    // The "make that switch visible" label the plan asks for: which box the
    // next Run actually reads from.
    let mode_label = match state.mode {
        Mode::Ask => "Next search translates the sentence above.",
        Mode::Query => "Next search uses this query as written.",
    };

    let mut rows: Vec<Element<'_, Message>> = vec![
        ui::cluster(vec![
            container(ui::input_mono_submit(
                "your query appears here once you run a search",
                &state.query_text,
                Message::QueryChanged,
                Message::Run,
            ))
            .width(Length::Fill)
            .into(),
            ui::button_secondary(Icon::Play, "Run", Message::Run),
        ])
        .into(),
        ui::caption(mode_label),
    ];

    let chips = state.response.as_ref().map(|r| part_chips(&r.dork.chips)).unwrap_or_default();
    if !chips.is_empty() {
        rows.push(ui::cluster(chips).wrap().into());
    }

    rows.push(add_operator_row(state));

    rows.push(
        ui::cluster(vec![
            container(ui::select(
                "Engine",
                SEARCH_ENGINES.to_vec(),
                Some(state.engine),
                Message::EngineChanged,
            ))
            .width(160)
            .into(),
            // Disabled (no handler) until there is a URL to open — see
            // `button_sized`'s doc comment on what `None` renders as.
            ui::button_sized(
                Some(Icon::Globe),
                format!("Open in {}", state.engine),
                ui::ButtonVariant::Default,
                ui::Size::Sm,
                state.response.as_ref().map(|_| Message::OpenResult),
            ),
        ])
        .into(),
    );

    ui::card_with_header(
        "Query",
        Some(ui::muted(
            "Editable — editing it (or removing a chip below) switches the next run to use \
             this text verbatim instead of re-asking above.",
        )),
        None,
        ui::stack(rows),
    )
}

/// A field picker, a value input, an Add button — the teaching surface for
/// the syntax typing an operator by hand would otherwise require. Sends
/// `add_field=<field>&add_value=<value>` against the current query; the
/// client never spells the operator itself (`AddField::wire`'s doc comment).
fn add_operator_row(state: &State) -> Element<'_, Message> {
    ui::cluster(vec![
        container(ui::select(
            "Add",
            AddField::ALL.to_vec(),
            Some(state.add_field),
            Message::AddFieldChanged,
        ))
        .width(260)
        .into(),
        container(ui::input_submit(
            state.add_field.placeholder(),
            &state.add_value,
            Message::AddValueChanged,
            Message::AddOperator,
        ))
        .width(Length::Fill)
        .into(),
        ui::button_secondary(Icon::Plus, "Add", Message::AddOperator),
    ])
    .into()
}

/// One removable chip per element the server sent — a click sends that
/// chip's own `token` back as `drop=` (`search.rs::run_search`). The server
/// (`DorkQuery::chips` in `search_dork.rs`) already built `token` with the
/// same grammar `drop_part` matches, so this is a straight map: no dork
/// operator syntax (`site:`, quoting, negation, …) is decided here.
/// `tone_for` is the only per-field judgement left on this side.
fn part_chips<'a>(chips: &[DorkChip]) -> Vec<Element<'a, Message>> {
    chips
        .iter()
        .map(|c| {
            ui::badge_button(
                format!("{}  ×", c.label),
                tone_for(&c.field),
                Message::RemovePart(c.token.clone()),
            )
        })
        .collect()
}

/// A chip's colour by which `DorkParts` field it came from — the only
/// judgement call the wire format (`DorkChip`) deliberately leaves client-side.
fn tone_for(field: &str) -> Tone {
    match field {
        "sites" => Tone::Success,
        "exclude" | "exclude_sites" => Tone::Warning,
        _ => Tone::Info,
    }
}

/// Recipe rows lead and read stronger (a plain-English sentence, an icon that
/// says "this is the reason"); operator rows follow, one per `DorkParts`
/// field actually in play, in muted detail-text. Mirrors how `search.rs`
/// orders `explanation` server-side — recipe rows first in the array too.
fn explanation_view(lines: &[DorkExplanationLine]) -> Element<'_, Message> {
    let mut rows: Vec<Element<'_, Message>> = Vec::new();
    for line in lines.iter().filter(|l| l.kind == "recipe") {
        rows.push(
            ui::cluster(vec![
                ui::icon::icon_tone(Icon::Sparkles, Tone::Info),
                ui::body(line.meaning.clone()),
            ])
            .into(),
        );
    }
    for line in lines.iter().filter(|l| l.kind != "recipe") {
        rows.push(
            ui::cluster(vec![
                container(ui::mono(line.label.clone())).width(200).into(),
                ui::muted(line.meaning.clone()),
            ])
            .into(),
        );
    }
    ui::stack(rows).into()
}

/// `configured: true`'s results area — title, domain, snippet, each row
/// opening its own URL. A genuinely empty `results` here is an ordinary
/// empty state, unlike the unconfigured case below.
fn results_card(results: &[SearchResult], total_estimate: Option<i64>) -> Element<'_, Message> {
    let subtitle = match total_estimate {
        Some(total) => format!("{} · about {total} total", ui::count(results.len(), "result", "results")),
        None => ui::count(results.len(), "result", "results"),
    };
    let body: Element<'_, Message> = if results.is_empty() {
        ui::empty_state("No matches. Try loosening the query.")
    } else {
        ui::stack(results.iter().map(result_row).collect()).into()
    };
    ui::card_with_header("Results", Some(ui::muted(subtitle)), None, body)
}

fn result_row(r: &SearchResult) -> Element<'_, Message> {
    ui::list_item(
        ui::stack(vec![
            ui::body(r.title.clone()),
            ui::caption(r.domain.clone()),
            ui::muted(r.snippet.clone()),
        ]),
        false,
        Message::OpenLink(r.url.clone()),
    )
}

/// `configured: false` — the default install. One quiet, dismissable line;
/// no error, no nag occupying the space results would otherwise fill. The
/// `Open in …` button above is already the whole answer for this install.
fn unconfigured_hint<'a>() -> Element<'a, Message> {
    ui::dismissible(
        ui::caption("Results can also show up right here — add a Search API key in Providers."),
        Message::DismissHint,
        Vec::new(),
    )
}

fn history_card(state: &State) -> Element<'_, Message> {
    let clear = (!state.history.is_empty()).then(|| ui::button_ghost(Icon::Trash, "Clear all", Message::HistoryClear));
    let body: Element<'_, Message> = if state.history.is_empty() {
        ui::empty_state("No searches yet.")
    } else {
        ui::stack(state.history.iter().map(history_row).collect()).into()
    };
    ui::card_with_header(
        "History",
        Some(ui::muted("Queries you've built and run. \"Opened\" means it went to your browser.")),
        clear,
        body,
    )
}

fn history_row(entry: &SearchHistoryEntry) -> Element<'_, Message> {
    ui::cluster(vec![
        container(ui::mono(entry.query.clone())).width(Length::Fill).into(),
        if entry.opened { ui::badge("opened", Tone::Success) } else { ui::badge("built", Tone::Info) },
        ui::button_ghost(Icon::Play, "Run", Message::HistorySelected(entry.id)),
        ui::button_ghost(Icon::Trash, "Delete", Message::HistoryDelete(entry.id)),
    ])
    .into()
}
