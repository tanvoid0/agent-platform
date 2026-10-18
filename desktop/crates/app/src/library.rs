//! Projects and Teams — the two catalog screens. Both are list + editor over a
//! small CRUD API, so they share this module and its save/delete plumbing.

use crate::domain::{err_string, non_empty};
use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::Task;

/// A project draft, or a team draft with its roster.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub id: Option<i64>,
    pub name: String,
    pub description: String,
    pub color: String,
    pub category: String,
    pub roles: Vec<RosterRole>,
}

impl Draft {
    fn from_project(p: &ProjectSummary) -> Self {
        Self {
            id: Some(p.id),
            name: p.name.clone(),
            description: p.description.clone().unwrap_or_default(),
            color: p.color.clone().unwrap_or_default(),
            ..Self::default()
        }
    }

    fn from_team(t: &TeamTemplateDetail) -> Self {
        Self {
            id: Some(t.id),
            name: t.name.clone(),
            description: t.description.clone().unwrap_or_default(),
            color: t.color.clone().unwrap_or_default(),
            category: t.category.clone().unwrap_or_default(),
            roles: t.roster.roles.clone(),
        }
    }

    fn project_body(&self) -> ProjectBody {
        ProjectBody {
            name: self.name.trim().to_string(),
            description: non_empty(&self.description),
            color: non_empty(&self.color),
        }
    }

    fn team_body(&self) -> TeamTemplateBody {
        TeamTemplateBody {
            name: self.name.trim().to_string(),
            description: non_empty(&self.description),
            color: non_empty(&self.color),
            category: non_empty(&self.category),
            roster: TeamRoster { roles: self.roles.clone() },
        }
    }
}

// ---------------------------------------------------------------------------
// Starting rosters
// ---------------------------------------------------------------------------

/// A curated starting roster, ported from the web library's template cards.
/// These are drafts, not server records: picking one fills the editor, and
/// nothing exists until it is saved.
pub struct Preset {
    pub name: &'static str,
    pub description: &'static str,
    color: &'static str,
    category: &'static str,
    roles: &'static [PresetRole],
}

struct PresetRole {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    /// `""` roots the role; anything else names its parent's preset id.
    parent: &'static str,
    accent: &'static str,
}

impl Preset {
    fn draft(&self) -> Draft {
        Draft {
            id: None,
            name: self.name.to_string(),
            description: self.description.to_string(),
            color: self.color.to_string(),
            category: self.category.to_string(),
            roles: self
                .roles
                .iter()
                .map(|r| RosterRole {
                    id: r.id.to_string(),
                    name: r.name.to_string(),
                    description: Some(r.description.to_string()),
                    // Text-only until the server routes other modalities.
                    modality: Some("text".into()),
                    parent_id: non_empty(r.parent),
                    accent_color: Some(r.accent.to_string()),
                })
                .collect(),
        }
    }
}

const GREEN: &str = "#16a34a";
const BLUE: &str = "#2563eb";
const PURPLE: &str = "#9333ea";
const ORANGE: &str = "#ea580c";
const GOLD: &str = "#ca8a04";
const RED: &str = "#dc2626";

pub const TEAM_PRESETS: &[Preset] = &[
    Preset {
        name: "Consultant workshop",
        description: "Single lead — quick experiments and written deliverables.",
        color: GREEN,
        category: "Workshop",
        roles: &[PresetRole {
            id: "lead",
            name: "Workshop lead",
            description: "Frames the ask, drafts the outcome, and hands off notes.",
            parent: "",
            accent: GREEN,
        }],
    },
    Preset {
        name: "Notepad mentorship",
        description: "Lead plus implementer and reviewer — guided delivery.",
        color: BLUE,
        category: "Mentorship",
        roles: &[
            PresetRole {
                id: "lead",
                name: "Mentor",
                description: "Keeps scope clear and reviews direction.",
                parent: "",
                accent: BLUE,
            },
            PresetRole {
                id: "junior",
                name: "Junior implementer",
                description: "Implements tasks and asks for checkpoints.",
                parent: "lead",
                accent: GREEN,
            },
            PresetRole {
                id: "reviewer",
                name: "Reviewer",
                description: "Sanity-checks changes and suggests fixes.",
                parent: "lead",
                accent: PURPLE,
            },
        ],
    },
    Preset {
        name: "Content sprint",
        description: "Parallel writers with a coordinating editor.",
        color: ORANGE,
        category: "Content",
        roles: &[
            PresetRole {
                id: "editor",
                name: "Editor",
                description: "Owns tone, deadlines, and final assembly.",
                parent: "",
                accent: ORANGE,
            },
            PresetRole {
                id: "a",
                name: "Writer A",
                description: "Drafts assigned sections.",
                parent: "editor",
                accent: BLUE,
            },
            PresetRole {
                id: "b",
                name: "Writer B",
                description: "Drafts assigned sections.",
                parent: "editor",
                accent: GREEN,
            },
            PresetRole {
                id: "fact",
                name: "Fact checker",
                description: "Traces claims to sources.",
                parent: "editor",
                accent: PURPLE,
            },
        ],
    },
    // The same four roles Studio's Ads tab uses when no team is named
    // (`server/src/ads.rs::default_roster`). Kept here as well so the roster is
    // something a user can fork and tune in the editor rather than a constant
    // only the server can see — the two lists feed different consumers and
    // neither breaks if the other changes.
    Preset {
        name: "Social media marketing",
        description: "Writes ads: one angle, one voice, one picture brief, the platform's rules.",
        color: PURPLE,
        category: "Marketing",
        roles: &[
            PresetRole {
                id: "strategist",
                name: "Campaign strategist",
                description:
                    "Owns the angle: which single benefit this ad is about, and who it is aimed \
                     at. Refuses to say three things at once.",
                parent: "",
                accent: PURPLE,
            },
            PresetRole {
                id: "copywriter",
                name: "Copywriter",
                description:
                    "Writes the caption and the call to action in the brand's voice. Leads with \
                     the hook, never with the company name.",
                parent: "strategist",
                accent: BLUE,
            },
            PresetRole {
                id: "art_director",
                name: "Art director",
                description:
                    "Decides what the picture shows and describes it as a diffusion prompt. Keeps \
                     the frame clear where text will sit.",
                parent: "strategist",
                accent: ORANGE,
            },
            PresetRole {
                id: "social_lead",
                name: "Social media lead",
                description:
                    "Knows each platform's conventions and limits. Picks hashtags people follow \
                     rather than padding the count.",
                parent: "strategist",
                accent: GREEN,
            },
        ],
    },
    Preset {
        name: "Autonomous product engineering",
        description: "Software-style tree: lead → senior + QA; backend chain to frontend.",
        color: BLUE,
        category: "Engineering",
        roles: &[
            PresetRole {
                id: "lead",
                name: "Team lead",
                description:
                    "Coordinates priorities, integrates work, requests human review when needed.",
                parent: "",
                accent: BLUE,
            },
            PresetRole {
                id: "senior",
                name: "Senior full-stack developer",
                description: "Owns architecture and splits work across the stack.",
                parent: "lead",
                accent: GREEN,
            },
            PresetRole {
                id: "qa",
                name: "QA & documentation",
                description: "Tests flows and keeps docs aligned.",
                parent: "lead",
                accent: PURPLE,
            },
            PresetRole {
                id: "backend",
                name: "Backend developer",
                description: "APIs, persistence, and integration points.",
                parent: "senior",
                accent: GOLD,
            },
            PresetRole {
                id: "frontend",
                name: "Frontend developer",
                description: "UI, accessibility, and client behavior.",
                parent: "backend",
                accent: RED,
            },
        ],
    },
];

#[derive(Default)]
pub struct State {
    /// `None` until the first fetch lands — an empty list means empty.
    pub projects: Option<Vec<ProjectSummary>>,
    pub teams: Option<Vec<TeamTemplateSummary>>,
    /// Full team detail for the open editor; the list only carries summaries.
    pub team_detail: Option<TeamTemplateDetail>,
    pub draft: Option<Draft>,
    pub selected_role: Option<String>,
    pub viewport: crate::graph::Viewport,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: crate::domain::Toast,
    /// A delete waiting on the in-app confirm dialog.
    pub confirm: Option<Confirm>,
}

/// What the confirm dialog is asking about, and the message a Yes sends.
pub struct Confirm {
    pub what: &'static str,
    pub then: Message,
}

impl State {
    /// Roster positions for the open team draft.
    pub fn roster_layout(&self) -> crate::graph::GraphLayout {
        let Some(draft) = &self.draft else { return Default::default() };
        crate::graph::roster_graph(&draft.roles)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    Refresh,
    ProjectsLoaded(Result<Vec<ProjectSummary>, String>),
    TeamsLoaded(Result<Vec<TeamTemplateSummary>, String>),
    TeamDetailLoaded(Result<Box<TeamTemplateDetail>, String>),

    NewProject,
    EditProject(i64),
    NewTeam,
    /// Index into [`TEAM_PRESETS`].
    NewTeamFromPreset(usize),
    EditTeam(i64),
    CancelEdit,

    NameChanged(String),
    DescriptionChanged(String),
    ColorChanged(String),
    CategoryChanged(String),

    AddRole,
    RemoveRole(String),
    SelectRole(Option<String>),
    RoleNameChanged(String, String),
    RoleDescriptionChanged(String, String),
    RoleParentChanged(String, Option<String>),
    Canvas(crate::graph::CanvasEvent),

    SaveProject,
    SaveTeam,
    DeleteProject(i64),
    DeleteTeam(i64),
    DeleteProjectConfirmed(i64),
    DeleteTeamConfirmed(i64),
    CancelConfirm,
    Done(Result<String, String>),
    DismissNotice,
}

impl From<crate::graph::CanvasEvent> for Message {
    fn from(event: crate::graph::CanvasEvent) -> Self {
        Message::Canvas(event)
    }
}

pub fn refresh(client: &Client) -> Task<Message> {
    let c1 = client.clone();
    let c2 = client.clone();
    Task::batch([
        Task::perform(
            async move { err_string(c1.projects().await).map(|r| r.projects) },
            Message::ProjectsLoaded,
        ),
        Task::perform(
            async move { err_string(c2.teams().await).map(|r| r.teams) },
            Message::TeamsLoaded,
        ),
    ])
}

/// Mutate the named role in the open draft.
fn edit_role(state: &mut State, id: &str, f: impl FnOnce(&mut RosterRole)) {
    if let Some(role) = state.draft.as_mut().and_then(|d| d.roles.iter_mut().find(|r| r.id == id)) {
        f(role);
    }
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        // A save or delete is already in flight; ignore repeat clicks.
        _ if state.busy
            && matches!(
                message,
                Message::SaveProject
                    | Message::SaveTeam
                    | Message::DeleteProject(_)
                    | Message::DeleteTeam(_)
                    | Message::DeleteProjectConfirmed(_)
                    | Message::DeleteTeamConfirmed(_)
            ) =>
        {
            Task::none()
        }
        Message::Refresh => refresh(client),
        Message::ProjectsLoaded(Ok(p)) => {
            state.error = None;
            state.projects = Some(p);
            Task::none()
        }
        Message::TeamsLoaded(Ok(t)) => {
            state.error = None;
            state.teams = Some(t);
            Task::none()
        }
        Message::TeamDetailLoaded(Ok(detail)) => {
            state.error = None;
            state.draft = Some(Draft::from_team(&detail));
            state.team_detail = Some(*detail);
            state.viewport = crate::graph::Viewport::default();
            Task::none()
        }
        Message::ProjectsLoaded(Err(e))
        | Message::TeamsLoaded(Err(e))
        | Message::TeamDetailLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }

        Message::NewProject => {
            state.draft = Some(Draft::default());
            state.team_detail = None;
            Task::none()
        }
        Message::EditProject(id) => {
            state.team_detail = None;
            state.draft =
                state.projects.iter().flatten().find(|p| p.id == id).map(Draft::from_project);
            Task::none()
        }
        Message::NewTeam => {
            state.team_detail = None;
            state.draft = Some(Draft { roles: vec![new_role(0)], ..Draft::default() });
            Task::none()
        }
        Message::NewTeamFromPreset(index) => {
            if let Some(preset) = TEAM_PRESETS.get(index) {
                state.team_detail = None;
                state.selected_role = None;
                state.draft = Some(preset.draft());
            }
            Task::none()
        }
        Message::EditTeam(id) => {
            let client = client.clone();
            Task::perform(
                async move { err_string(client.team_detail(id).await).map(Box::new) },
                Message::TeamDetailLoaded,
            )
        }
        Message::CancelEdit => {
            state.draft = None;
            state.team_detail = None;
            state.selected_role = None;
            Task::none()
        }

        Message::NameChanged(v) => {
            if let Some(d) = &mut state.draft {
                d.name = v;
            }
            Task::none()
        }
        Message::DescriptionChanged(v) => {
            if let Some(d) = &mut state.draft {
                d.description = v;
            }
            Task::none()
        }
        Message::ColorChanged(v) => {
            if let Some(d) = &mut state.draft {
                d.color = v;
            }
            Task::none()
        }
        Message::CategoryChanged(v) => {
            if let Some(d) = &mut state.draft {
                d.category = v;
            }
            Task::none()
        }

        Message::AddRole => {
            if let Some(d) = &mut state.draft {
                let role = new_role(d.roles.len());
                state.selected_role = Some(role.id.clone());
                d.roles.push(role);
            }
            Task::none()
        }
        Message::RemoveRole(id) => {
            if let Some(d) = &mut state.draft {
                d.roles.retain(|r| r.id != id);
                // Orphaned children would vanish from the tree layout, so
                // re-root them rather than leaving dangling parents.
                for role in &mut d.roles {
                    if role.parent_id.as_deref() == Some(id.as_str()) {
                        role.parent_id = None;
                    }
                }
            }
            if state.selected_role.as_deref() == Some(id.as_str()) {
                state.selected_role = None;
            }
            Task::none()
        }
        Message::SelectRole(id) => {
            state.selected_role = id;
            Task::none()
        }
        Message::RoleNameChanged(id, v) => {
            edit_role(state, &id, |r| r.name = v);
            Task::none()
        }
        Message::RoleDescriptionChanged(id, v) => {
            edit_role(state, &id, |r| r.description = non_empty(&v));
            Task::none()
        }
        Message::RoleParentChanged(id, parent) => {
            // A role cannot parent itself; anything else is left to the server.
            let parent = parent.filter(|p| p != &id);
            edit_role(state, &id, |r| r.parent_id = parent);
            Task::none()
        }
        Message::Canvas(event) => {
            use crate::graph::CanvasEvent;
            match event {
                CanvasEvent::Selected(id) => state.selected_role = Some(id),
                CanvasEvent::Panned(delta) => state.viewport.pan(delta),
                CanvasEvent::Zoomed(delta) => state.viewport.zoom(delta),
            }
            Task::none()
        }

        Message::SaveProject => {
            let Some(draft) = &state.draft else { return Task::none() };
            if draft.name.trim().is_empty() {
                state.error = Some("Name is required.".into());
                return Task::none();
            }
            state.busy = true;
            let body = draft.project_body();
            let id = draft.id;
            let client = client.clone();
            Task::perform(
                async move {
                    let saved = match id {
                        Some(id) => client.update_project(id, &body).await,
                        None => client.create_project(&body).await,
                    };
                    err_string(saved).map(|p| format!("Saved project “{}”.", p.name))
                },
                Message::Done,
            )
        }
        Message::SaveTeam => {
            let Some(draft) = &state.draft else { return Task::none() };
            if draft.name.trim().is_empty() {
                state.error = Some("Name is required.".into());
                return Task::none();
            }
            if draft.roles.iter().any(|r| r.name.trim().is_empty()) {
                state.error = Some("Every role needs a name.".into());
                return Task::none();
            }
            state.busy = true;
            let body = draft.team_body();
            let id = draft.id;
            let client = client.clone();
            Task::perform(
                async move {
                    let saved = match id {
                        Some(id) => client.update_team(id, &body).await,
                        None => client.create_team(&body).await,
                    };
                    err_string(saved).map(|t| format!("Saved team “{}”.", t.name))
                },
                Message::Done,
            )
        }
        Message::DeleteProject(id) => {
            state.confirm =
                Some(Confirm { what: "project", then: Message::DeleteProjectConfirmed(id) });
            Task::none()
        }
        Message::DeleteTeam(id) => {
            state.confirm = Some(Confirm { what: "team", then: Message::DeleteTeamConfirmed(id) });
            Task::none()
        }
        Message::CancelConfirm => {
            state.confirm = None;
            Task::none()
        }
        Message::DeleteProjectConfirmed(id) => {
            state.confirm = None;
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move { err_string(client.delete_project(id).await).map(|_| "Project deleted.".to_string()) },
                Message::Done,
            )
        }
        Message::DeleteTeamConfirmed(id) => {
            state.confirm = None;
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move { err_string(client.delete_team(id).await).map(|_| "Team deleted.".to_string()) },
                Message::Done,
            )
        }
        Message::Done(result) => {
            state.busy = false;
            match result {
                Ok(msg) => {
                    state.notice.set(msg);
                    state.draft = None;
                    state.team_detail = None;
                    state.selected_role = None;
                    refresh(client)
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Message::DismissNotice => {
            state.notice.clear();
            state.error = None;
            Task::none()
        }
    }
}

/// New roster rows get a stable synthetic id; the server keys roles by it.
fn new_role(index: usize) -> RosterRole {
    RosterRole {
        id: format!("role_{}", index + 1),
        name: String::new(),
        description: None,
        modality: None,
        parent_id: None,
        accent_color: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    #[test]
    fn presets_are_well_formed_rosters() {
        for preset in TEAM_PRESETS {
            let draft = preset.draft();
            assert!(draft.id.is_none(), "{}: a preset is a new draft", preset.name);
            let ids: Vec<&str> = draft.roles.iter().map(|r| r.id.as_str()).collect();
            for role in &draft.roles {
                assert_eq!(ids.iter().filter(|i| **i == role.id).count(), 1, "duplicate id");
                // A parent that names nothing would drop the role out of the
                // tree layout, which is how the roster is drawn and saved.
                if let Some(parent) = &role.parent_id {
                    assert!(ids.contains(&parent.as_str()), "{}: dangling parent", preset.name);
                }
            }
            assert_eq!(draft.roles.iter().filter(|r| r.parent_id.is_none()).count(), 1);
        }
    }

    #[test]
    fn picking_a_preset_fills_the_editor() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::NewTeamFromPreset(1));
        let draft = s.draft.expect("draft");
        assert_eq!(draft.name, TEAM_PRESETS[1].name);
        assert_eq!(draft.roles.len(), 3);
        // Out of range is a no-op, not a panic.
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::NewTeamFromPreset(999));
        assert!(s.draft.is_none());
    }

    #[test]
    fn removing_a_role_reparents_its_children() {
        let mut s = State::default();
        s.draft = Some(Draft {
            roles: vec![
                RosterRole { id: "a".into(), name: "A".into(), ..new_role(0) },
                RosterRole {
                    id: "b".into(),
                    name: "B".into(),
                    parent_id: Some("a".into()),
                    ..new_role(1)
                },
            ],
            ..Draft::default()
        });
        let _ = update(&mut s, &client(), Message::RemoveRole("a".into()));
        let roles = &s.draft.as_ref().unwrap().roles;
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].parent_id, None);
    }

    #[test]
    fn a_role_cannot_parent_itself() {
        let mut s = State::default();
        s.draft = Some(Draft { roles: vec![new_role(0)], ..Draft::default() });
        let id = s.draft.as_ref().unwrap().roles[0].id.clone();
        let _ = update(&mut s, &client(), Message::RoleParentChanged(id.clone(), Some(id)));
        assert_eq!(s.draft.unwrap().roles[0].parent_id, None);
    }

    /// A delete only arms the in-app dialog; nothing leaves until it is
    /// answered, and either answer takes the dialog back down.
    #[test]
    fn delete_asks_before_it_deletes() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::DeleteProject(7));
        assert!(matches!(s.confirm.as_ref().map(|c| c.what), Some("project")));
        assert!(!s.busy, "arming a dialog is not work in flight");

        let _ = update(&mut s, &client(), Message::CancelConfirm);
        assert!(s.confirm.is_none());

        let _ = update(&mut s, &client(), Message::DeleteTeam(7));
        let then = s.confirm.as_ref().unwrap().then.clone();
        assert!(matches!(then, Message::DeleteTeamConfirmed(7)));
        let _ = update(&mut s, &client(), then);
        assert!(s.confirm.is_none() && s.busy);
    }

    #[test]
    fn save_rejects_empty_names() {
        let mut s = State::default();
        s.draft = Some(Draft::default());
        let _ = update(&mut s, &client(), Message::SaveProject);
        assert!(s.error.is_some());
        assert!(!s.busy);

        let mut s = State::default();
        s.draft = Some(Draft { name: "T".into(), roles: vec![new_role(0)], ..Draft::default() });
        let _ = update(&mut s, &client(), Message::SaveTeam);
        assert!(s.error.as_deref().unwrap().contains("role"));
    }

    #[test]
    fn blank_optional_fields_are_sent_as_null() {
        let draft = Draft { name: "  P  ".into(), description: "  ".into(), ..Draft::default() };
        let body = draft.project_body();
        assert_eq!(body.name, "P");
        assert_eq!(body.description, None);
        assert_eq!(body.color, None);
    }
}
