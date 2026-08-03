//! Projects and Teams — the two catalog screens. Both are list + editor over a
//! small CRUD API, so they share this module and its save/delete plumbing.

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

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[derive(Default)]
pub struct State {
    pub projects: Vec<ProjectSummary>,
    pub teams: Vec<TeamTemplateSummary>,
    /// Full team detail for the open editor; the list only carries summaries.
    pub team_detail: Option<TeamTemplateDetail>,
    pub draft: Option<Draft>,
    pub selected_role: Option<String>,
    pub viewport: crate::graph::Viewport,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
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
    Refresh,
    ProjectsLoaded(Result<Vec<ProjectSummary>, String>),
    TeamsLoaded(Result<Vec<TeamTemplateSummary>, String>),
    TeamDetailLoaded(Result<Box<TeamTemplateDetail>, String>),

    NewProject,
    EditProject(i64),
    NewTeam,
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
    Done(Result<String, String>),
    DismissNotice,
}

impl From<crate::graph::CanvasEvent> for Message {
    fn from(event: crate::graph::CanvasEvent) -> Self {
        Message::Canvas(event)
    }
}

fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
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
        Message::Refresh => refresh(client),
        Message::ProjectsLoaded(Ok(p)) => {
            state.projects = p;
            Task::none()
        }
        Message::TeamsLoaded(Ok(t)) => {
            state.teams = t;
            Task::none()
        }
        Message::TeamDetailLoaded(Ok(detail)) => {
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
            state.draft = state.projects.iter().find(|p| p.id == id).map(Draft::from_project);
            Task::none()
        }
        Message::NewTeam => {
            state.team_detail = None;
            state.draft = Some(Draft { roles: vec![new_role(0)], ..Draft::default() });
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
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move { err_string(client.delete_project(id).await).map(|_| "Project deleted.".to_string()) },
                Message::Done,
            )
        }
        Message::DeleteTeam(id) => {
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
                    state.notice = Some(msg);
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
            state.notice = None;
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
