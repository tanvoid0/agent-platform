//! Coder rendering — the IDE shell.
//!
//! Four regions, fixed: an icon **rail** and the **sidebar** it switches on the
//! left, the **transcript and composer** in the middle, a **dock** under them,
//! and the **preview** strip on the right. Everything that used to stack as
//! cards under the transcript — the terminal, the file viewer, the checkpoint
//! diff — is a dock tab now, because three of them open at once pushed the
//! composer off the bottom of the window and each of the three is a thing you
//! look at *while* reading the transcript, not instead of it.
//!
//! Tool rows collapse to one line and expand to their output, because the two
//! questions are different: "what has it touched" is answered by scanning the
//! labels, "why did that fail" needs the text. A transcript that inlines every
//! `read_file` result is one where the model's own reasoning is unfindable.

use crate::coder::{Autonomy, Dock, Message, Pane, PlanMode, State, Turn};
use crate::coder_browser;
use crate::ui::{self, space, Icon, Tone};
use iced::widget::{
    column, container, markdown, row, scrollable, space as space_widget, text_editor, Row,
};
use iced::{Element, Length, Padding, Theme};

/// How tall the bottom dock is. Fixed rather than draggable: iced does not
/// report a widget's laid-out size back to `update`, so a splitter would need
/// its own measurement pass. `ponytail:` upgrade to a drag handle if the height
/// ever needs to differ per tab.
const DOCK_HEIGHT: f32 = 300.0;

pub fn view<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let mut panes: Vec<Element<'a, Message>> = vec![
        rail(state),
        ui::separator_vertical(),
        sidebar(state),
        ui::separator_vertical(),
        container(ui::page_custom(header(state), body(state, iced_theme)))
            .width(Length::Fill)
            .into(),
    ];
    if state.browser_open {
        panes.push(ui::separator_vertical());
        panes.push(browser(state));
    }
    row(panes).height(Length::Fill).into()
}

// ---------------------------------------------------------------------------
// Left: rail + sidebar
// ---------------------------------------------------------------------------

/// The icon column. Which *list* is on the left at the top, which *panel* is
/// open at the bottom — the same split as the two rows of the old header, but
/// as state you can see at a glance instead of labels you have to read.
fn rail(state: &State) -> Element<'_, Message> {
    let tab = |glyph: Icon, label: &'static str, pane: Pane| {
        ui::nav_icon_button(glyph, label, state.pane == pane, Message::SelectPane(pane))
    };
    let panel = |glyph: Icon, label: &'static str, on: bool, msg: Message| {
        ui::nav_icon_button(glyph, label, on, msg)
    };
    container(
        column![
            tab(Icon::Message, "Sessions", Pane::Sessions),
            tab(if state.files_open { Icon::FolderOpen } else { Icon::Folder }, "Files", Pane::Files),
            tab(Icon::Clock, "Checkpoints", Pane::Checkpoints),
            space_widget::vertical(),
            panel(Icon::Terminal, "Terminal", state.term.is_some(), Message::ToggleTerminal),
            panel(Icon::Globe, "Preview", state.browser_open, Message::ToggleBrowser),
        ]
        .spacing(space::XS)
        .align_x(iced::Alignment::Center),
    )
    .width(52)
    .padding(space::SM)
    .height(Length::Fill)
    .style(ui::theme::sidebar)
    .into()
}

fn sidebar(state: &State) -> Element<'_, Message> {
    let (head, items): (Element<'_, Message>, Vec<Element<'_, Message>>) = match state.pane {
        Pane::Sessions => (
            pane_head("Sessions", None),
            sessions(state),
        ),
        Pane::Files => (
            pane_head(
                "Files",
                // The turn refreshes this on its own; the button is for the
                // other writer — the user's own editor, or their git.
                Some(ui::icon_tip(Icon::Refresh, "Refresh files", Message::RefreshTree)),
            ),
            files(state),
        ),
        Pane::Checkpoints => (pane_head("Checkpoints", None), checkpoints(state)),
    };
    container(
        column![head, scrollable(column(items).spacing(2.0)).height(Length::Fill)]
            .spacing(space::XS),
    )
    .width(260)
    .padding(space::SM)
    .height(Length::Fill)
    .style(ui::theme::sidebar)
    .into()
}

fn pane_head<'a>(label: &'a str, action: Option<Element<'a, Message>>) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> =
        vec![ui::heading(label), space_widget::horizontal().into()];
    if let Some(action) = action {
        items.push(action);
    }
    ui::cluster(items).into()
}

/// Past sessions, newest first — read from the server rather than a local file:
/// coder threads are persisted server-side already, and a second copy here
/// would be the one that goes stale.
fn sessions(state: &State) -> Vec<Element<'_, Message>> {
    let mut items: Vec<Element<'_, Message>> =
        vec![ui::button_secondary(Icon::Plus, "New session", Message::New)];
    // Only once there is something to hand over. The pair reads as one choice —
    // start clean, or start clean carrying what this session learned.
    if state.thread_id.is_some() {
        items.push(ui::button_ghost(Icon::ArrowRight, "Hand off to a new one", Message::Fork));
    }
    if state.threads.is_empty() {
        items.push(if state.threads_loading {
            ui::caption(format!("{} Loading past sessions…", ui::spinner_char(state.frame)))
        } else {
            ui::caption("Past sessions appear here.")
        });
    }
    for t in &state.threads {
        items.push(
            ui::cluster(vec![
                container(ui::nav_item(
                    Icon::Message,
                    t.title.as_str(),
                    state.thread_id == Some(t.id),
                    Message::OpenThread(t.id),
                ))
                .width(Length::Fill)
                .into(),
                ui::icon_tip(
                    Icon::Trash,
                    if state.delete_armed == Some(t.id) {
                        "Click again to delete"
                    } else {
                        "Delete session"
                    },
                    Message::DeleteThread(t.id),
                ),
            ])
            .into(),
        );
    }
    items
}

/// The workspace as a tree — read-only, and only as deep as it has been opened.
///
/// Clicking a directory expands it; clicking a file opens it in the dock. There
/// is no editing here on purpose: see [`crate::coder_files`].
fn files(state: &State) -> Vec<Element<'_, Message>> {
    if state.root.is_none() {
        return vec![ui::caption("No folder open.")];
    }
    if state.tree.is_empty() {
        return vec![ui::caption("This folder is empty.")];
    }
    state
        .tree
        .iter()
        .map(|entry| {
            let open = state.expanded.contains(&entry.path);
            let (icon, message) = if entry.is_dir {
                (
                    if open { Icon::FolderOpen } else { Icon::Folder },
                    Message::ToggleDir(entry.path.clone()),
                )
            } else {
                (Icon::Scroll, Message::OpenFile(entry.path.clone()))
            };
            let selected = state.viewing.as_ref().is_some_and(|(p, _)| p == &entry.path);
            container(ui::nav_item(icon, entry.name.as_str(), selected, message))
                .padding(Padding {
                    // The indent *is* the path — the row shows a bare name.
                    left: entry.depth as f32 * 12.0,
                    ..Default::default()
                })
                .into()
        })
        .collect()
}

/// The agent's own file history for this folder — one entry per turn that
/// changed something, opening its diff in the dock.
fn checkpoints(state: &State) -> Vec<Element<'_, Message>> {
    if state.root.is_none() {
        return vec![ui::caption("No folder open.")];
    }
    if let Some(err) = &state.checkpoint_error {
        // A checkpoint failure never fails a turn, so it says so here rather
        // than in the banner that means the turn itself went wrong.
        return vec![ui::caption(format!("Not checkpointing: {err}"))];
    }
    if state.checkpoints.is_empty() {
        return vec![if state.checkpoints_loading {
            ui::caption(format!("{} Reading checkpoints…", ui::spinner_char(state.frame)))
        } else {
            ui::caption("A turn that changes a file becomes one.")
        }];
    }
    state
        .checkpoints
        .iter()
        .map(|c| {
            let open = state.reviewing.as_ref().is_some_and(|(sha, _)| sha == &c.sha);
            ui::list_item(
                column![ui::body(c.message.as_str()), ui::caption(format!("{} · {}", c.short(), c.when))]
                    .spacing(2.0),
                open,
                Message::ReviewCheckpoint(c.sha.clone()),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Right: the preview strip
// ---------------------------------------------------------------------------

/// The URL bar, and below it the hole the child window fills.
///
/// The empty container is not decoration: its height is what keeps the layout
/// honest about the strip [`coder_browser`] is painting into. Its contents are
/// only ever seen if the webview failed to appear.
fn browser(state: &State) -> Element<'_, Message> {
    let bar = container(
        ui::cluster(vec![
            ui::icon_tip(Icon::ArrowLeft, "Back", Message::BrowserBack),
            ui::icon_tip(Icon::ArrowRight, "Forward", Message::BrowserForward),
            ui::icon_tip(Icon::Refresh, "Reload", Message::BrowserReload),
            container(ui::input_submit(
                "localhost:3000",
                &state.browser_draft,
                Message::BrowserUrlChanged,
                Message::BrowserGo,
            ))
            .width(Length::Fill)
            .into(),
            ui::icon_tip(Icon::X, "Close browser", Message::ToggleBrowser),
        ]),
    )
    // `center_y(Fill)` would *set* the height to Fill and hand the bar half the
    // strip — which put the URL box inside the child window's rect, where every
    // click went to the page instead. Centering within the fixed height is the
    // whole point: this number is the child window's top edge.
    .padding(Padding::from([0.0, space::XS]))
    .center_y(coder_browser::BAR_HEIGHT);

    // Only drawn before anything has loaded — once a page is up the child
    // window covers this entirely. Which is also why the button below is only
    // ever pressable in exactly this state: no page, no child window over it.
    let empty = column![
        ui::empty_state_icon(Icon::Globe, "Preview a running dev server."),
        container(ui::button_secondary(
            Icon::Play,
            "Open localhost:3000",
            Message::BrowserOpenDefault,
        ))
        .center_x(Length::Fill),
    ]
    .spacing(space::SM);

    container(column![bar, container(empty).height(Length::Fill)])
    .width(coder_browser::WIDTH)
    .height(Length::Fill)
    .style(ui::theme::sidebar)
    .into()
}

// ---------------------------------------------------------------------------
// Center: header, transcript, dock, composer
// ---------------------------------------------------------------------------

fn header(state: &State) -> Element<'_, Message> {
    let folder: Element<'_, Message> = if state.root.is_some() {
        ui::badge_icon(Icon::Folder, state.root_label(), Tone::Neutral)
    } else {
        ui::badge_icon(Icon::Folder, "No folder open", Tone::Warning)
    };
    // The model belongs in the header rather than in Settings: it is the
    // single biggest determinant of whether a turn works at all, and the
    // server's default cannot hold a tool loop.
    let pickers = ui::model_pickers(
        state.provider_ids(),
        &state.provider,
        Message::ProviderChanged,
        state.model_options(),
        &state.model,
        Message::ModelChanged,
    );
    // Identity and the one workspace-changing action, alone on their own line:
    // a spacer safely pushes "Open folder" to the right here (`Length::Fill` is
    // fine in a plain row — see the wrap note below) and nothing here competes
    // with the folder path for room.
    let mut top_row: Vec<Element<'_, Message>> = vec![ui::title("Coder"), folder];
    // Rules that steer a turn have to be visible, or the turn that follows them
    // looks like a model with opinions.
    if state.agents_md {
        top_row.push(ui::badge_icon(Icon::Scroll, "AGENTS.md", Tone::Info));
    }
    let top = row![
        Row::with_children(top_row).spacing(space::SM).align_y(iced::Alignment::Center),
        space_widget::horizontal(),
        ui::button_outline(Icon::FolderOpen, "Open folder", Message::PickRoot),
    ]
    .spacing(space::SM)
    .align_y(iced::Alignment::Center);
    // What the turn is allowed to spend before it starts. The panel toggles
    // that used to sit here live in the rail now, so this line is config only.
    // Three states, not a checkbox: see [`PlanMode`]. A segmented control
    // rather than a third toggle beside the other two, because Off/Inline/Gate
    // are one choice and the toggles next to it are three separate ones.
    let budget = ui::cluster(vec![
        ui::segmented(
            PlanMode::ALL.map(|m| (m.label(), state.plan_mode == m, Message::SetPlanMode(m))),
        ),
        // The old Commands checkbox, grown a middle: Off/Ask/Allowlist/Auto is
        // one choice about how far the agent goes on its own, so it is one
        // control — see [`Autonomy`].
        ui::segmented(
            Autonomy::ALL.map(|a| (a.label(), state.autonomy == a, Message::SetAutonomy(a))),
        ),
        // Not config exactly — it changes what the dock does mid-turn — but it
        // belongs beside the other two: all three are "how this turn behaves".
        ui::toggle(Icon::Eye, "Follow", state.follow, Message::ToggleFollow(!state.follow)),
    ]);
    // `.wrap()` is a safety net for a narrower window, not the layout itself —
    // a `Length::Fill` child (a spacer, a rule) does not survive it in this
    // iced, measured against the row's full unwrapped extent rather than one
    // line, so it forces an early wrap instead of drawing a rule or pushing
    // something to the right. Neither group here is `Fill`, so this is safe.
    let bottom = Row::with_children(vec![pickers.into(), budget.into()])
        .spacing(space::LG)
        .align_y(iced::Alignment::Center)
        .wrap();

    column![top, bottom].spacing(space::SM).into()
}

fn body<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let transcript: Element<'_, Message> = if state.root.is_none() {
        ui::empty_state_action(
            Icon::Folder,
            "Open a folder to code in. The agent reads and writes inside it and \
             nowhere else.",
            ui::button_default(Icon::FolderOpen, "Open folder", Message::PickRoot),
        )
    } else if state.turns.is_empty() {
        ui::empty_state_icon(
            Icon::Message,
            "Ask for a change. The agent explores the folder itself before it edits.",
        )
    } else {
        ui::transcript(
            crate::coder::transcript_id(),
            state.turns.iter().enumerate().map(|(i, t)| turn(state, i, t, iced_theme)).collect(),
        )
    };

    let mut blocks: Vec<Element<'_, Message>> = Vec::new();
    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::DismissError, Vec::new()));
    }
    // Pinned above the transcript rather than inline in it: the point of the
    // list is that it stays readable while the rows it describes scroll away.
    if !state.todos.is_empty() {
        blocks.push(todos(state));
    }
    blocks.push(container(transcript).height(Length::Fill).into());
    if let Some(plan) = state.plan_card.as_ref() {
        blocks.push(plan_card(plan));
    }
    // `pending` outlives the decision being sent (so a failed send can put the
    // card back), so the card itself is gated on nothing being in flight.
    if let Some(pending) = state.pending.as_ref().filter(|_| !state.sending) {
        blocks.push(approval(state, &pending.command));
    }
    if let Some(sha) = state.last_turn.as_deref() {
        blocks.push(turn_review(state, sha));
    }
    if let Some(dock) = state.dock_shown() {
        blocks.push(dock_panel(state, dock));
    }
    blocks.push(composer(state));
    column(blocks).spacing(space::MD).height(Length::Fill).into()
}

/// The bottom dock: a tab strip over one panel, a fixed height tall.
fn dock_panel(state: &State, shown: Dock) -> Element<'_, Message> {
    let panel: Element<'_, Message> = match shown {
        Dock::Terminal => match state.term.as_ref() {
            Some(session) => terminal(session),
            None => ui::caption("The shell has closed."),
        },
        Dock::File => match state.viewing.as_ref() {
            Some((path, text)) => viewer(path, text),
            None => ui::caption("No file open."),
        },
        Dock::Diff => match state.reviewing.as_ref() {
            Some((sha, diff)) => review(state, sha, diff.as_ref()),
            None => ui::caption("No checkpoint open."),
        },
    };

    container(
        column![ui::cluster(strip_of(state, shown)), ui::separator(), panel]
            .spacing(space::SM),
    )
    .padding(space::SM)
    .height(DOCK_HEIGHT)
    .width(Length::Fill)
    .style(ui::theme::card)
    .into()
}

/// The dock's tab strip. Split out because it is built twice-shaped — tabs on
/// the left, the close for whatever is shown on the right.
fn strip_of(state: &State, shown: Dock) -> Vec<Element<'_, Message>> {
    let mut strip: Vec<Element<'_, Message>> = state
        .dock_tabs()
        .into_iter()
        .map(|d| {
            let (glyph, label) = match d {
                Dock::Terminal => (Icon::Terminal, "Terminal"),
                Dock::File => (Icon::Scroll, "File"),
                Dock::Diff => (Icon::Clock, "Diff"),
            };
            ui::toggle(glyph, label, d == shown, Message::SelectDock(d))
        })
        .collect();
    strip.push(space_widget::horizontal().into());
    strip.push(match shown {
        Dock::Terminal => ui::button_ghost(Icon::X, "Close", Message::ToggleTerminal),
        Dock::File => ui::button_ghost(Icon::X, "Close", Message::CloseFile),
        Dock::Diff => ui::button_ghost(Icon::X, "Close", Message::CloseReview),
    });
    strip
}

/// A real terminal grid — `iced_term` over `alacritty_terminal` — so what is
/// drawn here is what a terminal draws: colour, the cursor where the program
/// put it, the alternate screen, selection. It takes the keyboard when focused.
fn terminal(session: &crate::coder_term::Session) -> Element<'_, Message> {
    container(iced_term::TerminalView::show(&session.0).map(Message::Term))
        .height(Length::Fill)
        .into()
}

/// One file, as it is on disk right now.
fn viewer<'a>(path: &'a std::path::Path, text: &'a Result<String, String>) -> Element<'a, Message> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let body: Element<'a, Message> = match text {
        // Not an error banner: asking to read a PNG is a click, not a fault.
        Err(why) => ui::muted(why.as_str()),
        Ok(content) => scrollable(ui::code(ui::mono(content.as_str()))).height(Length::Fill).into(),
    };
    column![ui::badge_icon(Icon::Scroll, name, Tone::Neutral), body]
        .spacing(space::XS)
        .height(Length::Fill)
        .into()
}

/// What one checkpoint changed, and the way back to it.
fn review<'a>(state: &'a State, sha: &'a str, diff: Option<&'a String>) -> Element<'a, Message> {
    let armed = state.restore_armed.as_deref() == Some(sha);
    let label = state
        .checkpoints
        .iter()
        .find(|c| c.sha == sha)
        .map(|c| format!("{} · {}", c.short(), c.when))
        .unwrap_or_else(|| sha.to_string());

    let head: Element<'_, Message> = ui::cluster(vec![
        ui::badge_icon(Icon::Clock, label, Tone::Neutral),
        space_widget::horizontal().into(),
        // Armed on the first press, done on the second. `reset --hard` takes
        // the files back to here and takes everything since with them — later
        // turns, and whatever the user typed in their own editor.
        ui::button_destructive(
            Icon::RotateCcw,
            if armed { "Confirm — discards changes since" } else { "Restore this checkpoint" },
            Message::RestoreCheckpoint(sha.to_string()),
        ),
    ])
    .into();

    let body: Element<'_, Message> = match diff {
        None => ui::caption("Reading the diff…"),
        Some(text) => scrollable(ui::code(ui::mono(text.as_str())))
            .id(iced::widget::Id::new("coder-diff"))
            .height(Length::Fill)
            .into(),
    };
    let mut rows = column![head].spacing(space::XS);
    if let Some(changes) = state.changes.as_ref() {
        rows = rows.push(changed_files(changes));
    }
    rows.push(body).height(Length::Fill).into()
}

/// The files one checkpoint touched, each with the way to undo just that one.
///
/// Above the patch rather than inside it: the patch answers "what changed", this
/// answers "keep which of it" — and keeping most of a turn while dropping one
/// file is the common shape of reviewing an agent's work.
fn changed_files(changes: &crate::coder_git::Changes) -> Element<'_, Message> {
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    for change in &changes.files {
        let (tone, verb) = match change.status {
            'A' => (Tone::Success, "added"),
            'D' => (Tone::Danger, "deleted"),
            _ => (Tone::Info, "changed"),
        };
        let mut cells: Vec<Element<'_, Message>> = vec![
            ui::badge(verb, tone),
            ui::mono(change.path.as_str()).into(),
            space_widget::horizontal().into(),
        ];
        // The baseline has nothing before it: "revert to before the baseline"
        // would delete files the user had before the agent ever ran.
        if changes.revertable {
            cells.push(ui::button_ghost(
                Icon::RotateCcw,
                "Revert",
                Message::RevertFile(change.path.clone()),
            ));
        }
        items.push(ui::cluster(cells).into());
    }
    if changes.hidden > 0 {
        items.push(ui::caption(ui::count(changes.hidden, "more file", "more files")));
    }
    // Bounded, and scrolls: a turn can touch more files than the dock is tall.
    scrollable(column(items).spacing(2.0)).height(Length::Fixed(96.0)).into()
}

/// "That turn changed files" — one line above the composer, gone once it has
/// been looked at or dismissed.
///
/// The timeline in the sidebar says the same thing, but only to someone with the
/// Checkpoints pane open; this says it where the user already is, which is the
/// composer they are about to type the next prompt into.
fn turn_review<'a>(state: &'a State, sha: &'a str) -> Element<'a, Message> {
    let when = state
        .checkpoints
        .iter()
        .find(|c| c.sha == sha)
        .map(|c| c.message.as_str())
        .unwrap_or("the last turn");
    let bar: Element<'a, Message> = ui::cluster(vec![
        ui::badge_icon(Icon::Clock, "changed files", Tone::Info),
        ui::caption(when),
        space_widget::horizontal().into(),
        ui::button_secondary(
            Icon::Eye,
            "Review changes",
            Message::ReviewCheckpoint(sha.to_string()),
        ),
        // The other reader. Pick a stronger model in the header first and this
        // is a second opinion on a cheap model's work, which is the whole point
        // of it being a button rather than something typed.
        ui::button_ghost(Icon::Search, "Ask the model", Message::ReviewTurn(sha.to_string())),
        ui::button_ghost(Icon::X, "Dismiss", Message::CloseReview),
    ])
    .into();
    ui::card(bar)
}

/// The gate in front of `run_command`. It shows the command, not the tool's
/// name: a prompt reading "the agent wants to run a command" is one people
/// approve without reading, which is the same as having no gate.
///
/// Which is also why an unreadable call offers no Run button at all. A model
/// that leaks its tool syntax as prose gets salvaged server-side with whatever
/// arguments survived — seen live producing an empty command under a live Run
/// button, which is the one thing this card must never be.
fn approval<'a>(state: &'a State, command: &'a str) -> Element<'a, Message> {
    let unreadable = command.is_empty();
    let body: Element<'_, Message> = if unreadable {
        ui::muted(
            "The model asked to run a command but did not say what. Nothing will be run.",
        )
    } else {
        ui::code(ui::mono(command.to_string()))
    };
    // What the "always" would actually be, spelled out. A button that saves an
    // invisible rule is one people press once and then cannot explain their
    // agent with — and the rule is wider than the command it came from.
    let rule = crate::coder::rule_for(command);
    let always = (!unreadable && state.autonomy != Autonomy::Auto && !rule.is_empty())
        .then(|| ui::button_secondary(Icon::Check, format!("Always allow {rule}"), Message::AlwaysAllow));
    ui::approval_extra(
        if unreadable { "Unreadable command" } else { "Run this command?" },
        if unreadable { Tone::Danger } else { Tone::Warning },
        vec![body],
        if unreadable { "Dismiss" } else { "No" },
        Message::Decide(false),
        (!unreadable).then_some(Message::Decide(true)),
        always,
    )
}

/// The agent's own checklist, as `update_todos` last posted it. Badges in a
/// wrapping row rather than a column of lines: this sits above the transcript
/// on every turn that has one, and five vertical rows there is five rows the
/// conversation does not get.
fn todos(state: &State) -> Element<'_, Message> {
    let done = state.todos.iter().filter(|t| t.done).count();
    let mut cells: Vec<Element<'_, Message>> =
        vec![ui::caption(format!("{done}/{} done", state.todos.len()))];
    for item in &state.todos {
        let mut label: String = item.text.chars().take(48).collect();
        if label.chars().count() < item.text.chars().count() {
            label.push('…');
        }
        cells.push(match item.done {
            true => ui::badge_icon(Icon::Check, label, Tone::Success),
            false => ui::badge_icon(Icon::Clock, label, Tone::Neutral),
        });
    }
    ui::card(Row::with_children(cells).spacing(space::XS).align_y(iced::Alignment::Center).wrap())
}

/// The plan gate: what the agent says it will do, before it can do any of it.
///
/// Editable, which is the whole reason this costs a round trip over
/// [`PlanMode::Inline`] — the text in the box is what the next turn is handed,
/// so a wrong step is fixed here instead of being undone afterwards.
fn plan_card(content: &text_editor::Content) -> Element<'_, Message> {
    let head = ui::cluster(vec![
        ui::badge_icon(Icon::ListChecks, "Plan", Tone::Info),
        space_widget::horizontal().into(),
        ui::button_ghost(Icon::X, "Discard", Message::PlanDiscard),
        ui::button_default(Icon::Play, "Run", Message::PlanRun),
    ]);
    ui::card(
        column![
            head,
            ui::caption("Edit it before it runs — this text is what the agent is handed."),
            ui::code(text_editor(content).on_action(Message::PlanEdited).height(180.0)),
        ]
        .spacing(space::SM),
    )
}

fn composer(state: &State) -> Element<'_, Message> {
    let typed = !state.draft.trim().is_empty();
    // Same composer as E.V.'s, with a different trailing control: this one can
    // also be waiting on a folder or on an approval, which "Send" would lie about.
    // Stop sits beside the spinner rather than replacing it: what is running and
    // the way to end it are two facts, and a control that swaps between them
    // makes the user wait to find out which one they are looking at.
    let trailing: Vec<Element<'_, Message>> = if state.sending {
        let mut controls = vec![ui::badge_spinner(state.frame, "working…", Tone::Info)];
        // Only with something typed: "Stop & send" over an empty box would send
        // nothing, which is a slower Stop.
        if typed {
            controls.push(ui::button_secondary(Icon::Send, "Stop & send", Message::StopAndSend));
        }
        controls.push(ui::button_destructive(Icon::Stop, "Stop", Message::Stop));
        controls
    } else if state.root.is_none() {
        vec![ui::badge("waiting", Tone::Neutral)]
    } else if state.would_queue() {
        // Parked on a decision: Enter still takes a follow-up, it just goes
        // behind the turn that is waiting on the user.
        vec![ui::button_secondary(Icon::Plus, "Queue", Message::Send)]
    } else {
        vec![ui::button_default(Icon::Send, "Send", Message::Send)]
    };
    let input = ui::composer(
        match (state.root.is_some(), state.would_queue()) {
            (false, _) => "Open a folder first…",
            // Naming what Enter does now, because it does something different.
            (true, true) => "Queue a follow-up…",
            (true, false) => "What should change?",
        },
        &state.draft,
        Message::DraftChanged,
        Message::Send,
        trailing,
    );
    let mut rows = column![].spacing(space::XS);
    if !state.queue.is_empty() {
        rows = rows.push(queued(state));
    }
    rows = rows.push(input);
    if !state.would_queue() {
        return ui::card(rows);
    }
    // What it is waiting on, and for how long. A local model can spend minutes
    // inside one step, and "working…" on its own cannot tell a slow read from a
    // dead connection — the clock is what makes the difference visible.
    let status: Element<'_, Message> =
        ui::caption(format!("{}… {}s", state.activity(), state.elapsed));
    ui::card(rows.push(status))
}

/// Follow-ups waiting their turn. Each is a button, and pressing it takes that
/// one back into the composer — the queue's only exit, and the reason it needs no
/// separate remove: nothing typed can be dropped by a misclick.
fn queued(state: &State) -> Element<'_, Message> {
    let mut cells: Vec<Element<'_, Message>> =
        vec![ui::caption(ui::count(state.queue.len(), "queued", "queued"))];
    for (i, text) in state.queue.iter().enumerate() {
        let mut label: String = text.chars().take(48).collect();
        if label.chars().count() < text.chars().count() {
            label.push('…');
        }
        cells.push(ui::badge_button(label, Tone::Neutral, Message::Unqueue(i)));
    }
    // Wraps rather than scrolls: the chips are short, and a scroll region inside
    // the composer is one the user has to find before they can read it.
    Row::with_children(cells).spacing(space::XS).align_y(iced::Alignment::Center).wrap().into()
}

fn turn<'a>(state: &'a State, idx: usize, turn: &'a Turn, iced_theme: &Theme) -> Element<'a, Message> {
    match turn {
        Turn::User(text) => {
            // The message that was sent carries the @mentioned files; the row
            // shows the sentence and says the rest is there. Same split live and
            // on rebuild, because the marker is in the persisted text.
            let (typed, inlined) = match text.split_once(crate::coder::MENTION_MARKER) {
                Some((head, _)) => (head, true),
                None => (text.as_str(), false),
            };
            let body: Element<'_, Message> = match inlined {
                false => ui::body(typed),
                true => column![ui::body(typed), ui::caption("+ the @mentioned files, inlined")]
                    .spacing(space::XS)
                    .into(),
            };
            ui::turn("You", Tone::Neutral, true, body)
        }
        Turn::Assistant { md, .. } => ui::turn(
            "Coder",
            Tone::Info,
            false,
            markdown::view(md, markdown::Settings::from(iced_theme)).map(Message::LinkClicked),
        ),
        Turn::Tool { label, result } => tool_row(state, idx, label, result.as_ref()),
    }
}

fn tool_row<'a>(
    state: &'a State,
    idx: usize,
    label: &'a str,
    result: Option<&'a Result<String, String>>,
) -> Element<'a, Message> {
    // A call still running gets the spinner rather than a static icon — it can
    // sit here for minutes, and "running…" alone reads the same at second one
    // and second ninety.
    let status_badge: Element<'_, Message> = match result {
        Some(Ok(_)) => ui::badge_icon(ui::tone_icon(Tone::Success), "done", Tone::Success),
        Some(Err(_)) => ui::badge_icon(ui::tone_icon(Tone::Danger), "failed", Tone::Danger),
        None => ui::badge_spinner(state.frame, "running…", Tone::Info),
    };
    let head = row![status_badge, ui::mono(label), space_widget::horizontal()]
        .spacing(space::SM)
        .align_y(iced::Alignment::Center);

    let Some(result) = result else {
        return container(head).padding(space::XS).into();
    };
    let text = match result {
        Ok(t) => t.as_str(),
        Err(t) => t.as_str(),
    };
    let open = state.open_tools.contains(&idx);
    let toggle = ui::button_ghost(
        if open { Icon::EyeOff } else { Icon::Eye },
        if open { "Hide output" } else { "Output" },
        Message::ToggleTool(idx),
    );
    let mut head = row![head, toggle].spacing(space::SM).align_y(iced::Alignment::Center);
    // A command the agent ran, offered back to the user's own shell. This is
    // the answer to "it failed and I want to see why": the same command, in the
    // same folder, where it can be edited, re-run, and answer a prompt.
    if let Some(command) = label.strip_prefix("$ ").filter(|c| !c.trim().is_empty()) {
        head = head.push(ui::button_ghost(
            Icon::Terminal,
            "Run in terminal",
            Message::SendToTerminal(command.to_string()),
        ));
    }
    if !open {
        return container(head).padding(space::XS).into();
    }
    container(column![head, ui::code(ui::mono(text))].spacing(space::XS))
        .padding(space::XS)
        .into()
}
