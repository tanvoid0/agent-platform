//! Projects and Teams rendering. Both screens share one state; which list and
//! editor to show is decided by the caller's `Kind`.

use crate::domain;
use crate::library::{Message, State};
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::RosterRole;
use iced::widget::{column, container, scrollable};
use iced::{Element, Length};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Projects,
    Teams,
}

pub fn view(state: &State, kind: Kind) -> Element<'_, Message> {
    let (title, subtitle, new_label, new_msg) = match kind {
        Kind::Projects => (
            "Projects",
            "Group runs so their goals, teams and history stay together.",
            "New project",
            Message::NewProject,
        ),
        Kind::Teams => (
            "Teams",
            "Reusable rosters the planner draws subagents from.",
            "New team",
            Message::NewTeam,
        ),
    };

    let mut blocks: Vec<Element<'_, Message>> = Vec::new();
    if let Some(err) = &state.error {
        blocks.push(dismissible(ui::alert_error(err.clone())));
    }

    blocks.push(match (&state.draft, kind) {
        (Some(_), Kind::Projects) => project_editor(state),
        (Some(_), Kind::Teams) => team_editor(state),
        (None, Kind::Projects) => project_list(state),
        (None, Kind::Teams) => team_list(state),
    });

    let actions = match (&state.draft, state.busy) {
        (Some(_), _) => None,
        // A delete is in flight: the list has no other place to say so.
        (None, true) => Some(ui::badge("working…", Tone::Info)),
        (None, false) => Some(ui::button_default(Icon::Plus, new_label, new_msg)),
    };
    let page = ui::page(title, Some(ui::muted(subtitle)), actions, ui::stack_lg(blocks));

    match &state.confirm {
        None => page,
        Some(confirm) => ui::modal(
            page,
            ui::confirm_dialog(
                format!("Delete this {}?", confirm.what),
                "This cannot be undone.",
                vec![
                    ui::button_ghost(Icon::X, "Cancel", Message::CancelConfirm),
                    ui::button_destructive(Icon::Trash, "Delete", confirm.then.clone()),
                ],
            ),
            420.0,
        ),
    }
}

fn dismissible(inner: Element<'_, Message>) -> Element<'_, Message> {
    ui::cluster(vec![
        container(inner).width(Length::Fill).into(),
        ui::button_ghost(Icon::X, "Dismiss", Message::DismissNotice),
    ])
    .into()
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

fn project_list(state: &State) -> Element<'_, Message> {
    let Some(projects) = &state.projects else {
        return ui::card(ui::empty_state_icon(Icon::Clock, "Loading…"));
    };
    if projects.is_empty() {
        return ui::card(ui::empty_state_icon(Icon::Folder, "No projects yet."));
    }
    let rows: Vec<Element<'_, Message>> = projects
        .iter()
        .map(|p| {
            ui::card(ui::cluster(vec![
                ui::stack(vec![
                    ui::body(p.name.clone()),
                    ui::caption(p.description.clone().unwrap_or_else(|| "—".into())),
                ])
                .into(),
                ui::spacer(),
                ui::caption(domain::relative_time(&p.updated_at).unwrap_or_default()),
                ui::button_outline(Icon::Pencil, "Edit", Message::EditProject(p.id)),
                ui::button_destructive(Icon::Trash, "Delete", Message::DeleteProject(p.id)),
            ]))
        })
        .collect();
    scrollable(ui::stack(rows)).height(Length::Fill).into()
}

/// Curated starting rosters. An empty library is the case that most needs them,
/// so they sit above the list either way rather than only in the empty state.
fn team_presets<'a>() -> Element<'a, Message> {
    let cards: Vec<Element<'a, Message>> = crate::library::TEAM_PRESETS
        .iter()
        .enumerate()
        .map(|(i, preset)| {
            ui::card(ui::stack(vec![
                ui::body(preset.name),
                ui::caption(preset.description),
                ui::button_outline(Icon::Plus, "Use", Message::NewTeamFromPreset(i)),
            ]))
        })
        .collect();
    ui::section(
        "Start from a template",
        Some(ui::muted("Fills the editor — nothing is saved until you save it.")),
        ui::cluster(cards).align_y(iced::Alignment::Start),
    )
}

fn team_list(state: &State) -> Element<'_, Message> {
    let Some(teams) = &state.teams else {
        return ui::card(ui::empty_state_icon(Icon::Clock, "Loading…"));
    };
    if teams.is_empty() {
        return ui::stack_lg(vec![
            ui::card(ui::empty_state_icon(Icon::Users, "No teams yet.")),
            team_presets(),
        ])
        .into();
    }
    let rows: Vec<Element<'_, Message>> = teams
        .iter()
        .map(|t| {
            ui::card(ui::cluster(vec![
                ui::stack(vec![
                    ui::body(t.name.clone()),
                    ui::caption(t.description.clone().unwrap_or_else(|| "—".into())),
                ])
                .into(),
                ui::spacer(),
                ui::caption(domain::relative_time(&t.updated_at).unwrap_or_default()),
                ui::badge(ui::count(t.role_count as usize, "role", "roles"), Tone::Neutral),
                ui::button_outline(Icon::Pencil, "Edit", Message::EditTeam(t.id)),
                ui::button_destructive(Icon::Trash, "Delete", Message::DeleteTeam(t.id)),
            ]))
        })
        .collect();
    scrollable(ui::stack_lg(vec![ui::stack(rows).into(), team_presets()]))
        .height(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Editors
// ---------------------------------------------------------------------------

fn project_editor(state: &State) -> Element<'_, Message> {
    let draft = state.draft.as_ref().expect("editor requires a draft");
    ui::card_with_header(
        if draft.id.is_some() { "Edit project" } else { "New project" },
        None,
        Some(save_actions(state, Message::SaveProject)),
        ui::stack(vec![
            ui::field("Name", ui::input("Project name", &draft.name, Message::NameChanged)),
            ui::field(
                "Description",
                ui::input("What is this project for?", &draft.description, Message::DescriptionChanged),
            ),
            ui::field("Color", ui::input("#4285F4", &draft.color, Message::ColorChanged)),
        ]),
    )
}

fn save_actions(state: &State, save: Message) -> Element<'_, Message> {
    let mut buttons: Vec<Element<'_, Message>> = Vec::new();
    if state.busy {
        buttons.push(ui::badge("saving…", Tone::Info));
    }
    buttons.push(ui::button_default(Icon::Save, "Save", save));
    buttons.push(ui::button_ghost(Icon::X, "Cancel", Message::CancelEdit));
    ui::cluster(buttons).into()
}

fn team_editor(state: &State) -> Element<'_, Message> {
    let draft = state.draft.as_ref().expect("editor requires a draft");

    let details = ui::card_with_header(
        if draft.id.is_some() { "Edit team" } else { "New team" },
        None,
        Some(save_actions(state, Message::SaveTeam)),
        ui::stack(vec![
            ui::field("Name", ui::input("Team name", &draft.name, Message::NameChanged)),
            ui::field(
                "Description",
                ui::input("What does this team do?", &draft.description, Message::DescriptionChanged),
            ),
            ui::field("Category", ui::input("engineering", &draft.category, Message::CategoryChanged)),
            ui::field("Color", ui::input("#4285F4", &draft.color, Message::ColorChanged)),
        ]),
    );

    let layout = state.roster_layout();
    let canvas: Element<'_, Message> = if layout.nodes.is_empty() {
        ui::empty_state_icon(Icon::Users, "No roles yet.")
    } else {
        iced::widget::canvas(crate::graph::DagCanvas {
            layout,
            viewport: state.viewport,
            selected: state.selected_role.clone(),
        })
        .width(Length::Fill)
        .height(260)
        .into()
    };

    let roles: Vec<Element<'_, Message>> =
        draft.roles.iter().map(|r| role_row(state, r, &draft.roles)).collect();

    let roster = ui::card_with_header(
        "Roster",
        Some(ui::muted("Reporting lines come from each role's parent.")),
        Some(ui::button_secondary(Icon::Plus, "Add role", Message::AddRole)),
        column![canvas, ui::separator(), ui::stack(roles)].spacing(space::MD),
    );

    ui::stack_lg(vec![details, roster]).into()
}

fn role_row<'a>(
    state: &'a State,
    role: &'a RosterRole,
    all: &'a [RosterRole],
) -> Element<'a, Message> {
    let id = role.id.clone();
    let selected = state.selected_role.as_deref() == Some(role.id.as_str());

    // Parent options: every other role, plus an explicit "no parent".
    let mut names = vec![NO_PARENT.to_string()];
    names.extend(all.iter().filter(|r| r.id != role.id).map(|r| label_for(r)));
    let current = role
        .parent_id
        .as_deref()
        .and_then(|p| all.iter().find(|r| r.id == p))
        .map(label_for)
        .or_else(|| Some(NO_PARENT.to_string()));

    let by_label: Vec<(String, String)> =
        all.iter().map(|r| (label_for(r), r.id.clone())).collect();
    let id_for_parent = id.clone();
    let id_for_name = id.clone();
    let id_for_desc = id.clone();

    let inner = ui::stack(vec![
        ui::cluster(vec![
            container(ui::input("Role name", &role.name, move |v| {
                Message::RoleNameChanged(id_for_name.clone(), v)
            }))
            .width(Length::Fill)
            .into(),
            container(ui::select(
                "Reports to",
                names,
                current,
                move |label: String| {
                    Message::RoleParentChanged(
                        id_for_parent.clone(),
                        by_label.iter().find(|(l, _)| *l == label).map(|(_, id)| id.clone()),
                    )
                },
            ))
            .width(220)
            .into(),
            ui::button_destructive(Icon::Trash, "Remove", Message::RemoveRole(id.clone())),
        ])
        .into(),
        ui::input(
            "What this role is responsible for",
            role.description.as_deref().unwrap_or(""),
            move |v| Message::RoleDescriptionChanged(id_for_desc.clone(), v),
        ),
    ]);

    ui::list_item(inner, selected, Message::SelectRole(Some(role.id.clone())))
}

const NO_PARENT: &str = "— no parent —";

fn label_for(role: &RosterRole) -> String {
    if role.name.trim().is_empty() {
        role.id.clone()
    } else {
        role.name.clone()
    }
}
