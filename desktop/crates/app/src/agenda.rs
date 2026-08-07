//! Agenda — the personal-assistant dashboard the server has served since the
//! Phase 7 roadmap and no native screen ever showed (it was a web-only surface,
//! deleted with `web/`).
//!
//! One project at a time: its assistant board sliced by horizon, plus the
//! reviewer's pending suggestions. Cards are *completed* here rather than moved
//! — Plans is the screen for moving things through columns.

use crate::agenda_chat;
use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::Task;

#[derive(Debug)]
pub struct State {
    pub projects: Vec<ProjectSummary>,
    pub project: Option<i64>,
    /// One of [`ASSISTANT_HORIZONS`].
    pub horizon: String,
    pub dashboard: Option<AssistantDashboard>,
    pub reviews: Vec<AssistantReview>,
    /// A first fetch is in flight — the difference between "nothing here" and
    /// "not yet".
    pub loading: bool,
    pub busy: bool,
    pub error: Option<String>,
    /// The planning chat beside the board. It proposes what goes on the board,
    /// so it follows this screen's project rather than picking its own.
    pub chat: agenda_chat::State,
}

impl Default for State {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            project: None,
            horizon: ASSISTANT_HORIZONS[0].to_string(),
            dashboard: None,
            reviews: Vec::new(),
            loading: false,
            busy: false,
            error: None,
            chat: agenda_chat::State::default(),
        }
    }
}

impl State {
    pub fn category(&self, id: Option<i64>) -> Option<&TodoCategory> {
        let (board, id) = (self.dashboard.as_ref()?, id?);
        board.categories.iter().find(|c| c.id == id)
    }

    pub fn project_name(&self, id: i64) -> Option<&str> {
        self.projects.iter().find(|p| p.id == id).map(|p| p.name.as_str())
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    Refresh,
    ProjectsLoaded(Result<Vec<ProjectSummary>, String>),
    SelectProject(i64),
    SetHorizon(String),
    DashboardLoaded(Result<Box<AssistantDashboard>, String>),
    ReviewsLoaded(Result<Vec<AssistantReview>, String>),
    Complete(i64),
    RunReview,
    ApplyReview(i64),
    DismissReview(i64),
    /// Any write finished; the dashboard is refetched rather than patched.
    Done(Result<(), String>),
    Dismiss,
    Chat(agenda_chat::Message),
}

fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

pub fn refresh(client: &Client) -> Task<Message> {
    let c = client.clone();
    Task::perform(
        async move { err_string(c.projects().await).map(|r| r.projects) },
        Message::ProjectsLoaded,
    )
}

fn load(client: &Client, project: i64, horizon: &str) -> Task<Message> {
    let (c, d) = (client.clone(), client.clone());
    let horizon = horizon.to_string();
    Task::batch([
        Task::perform(
            async move { err_string(c.assistant_dashboard(project, &horizon).await).map(Box::new) },
            Message::DashboardLoaded,
        ),
        Task::perform(
            async move { err_string(d.assistant_pending_reviews(project).await).map(|r| r.reviews) },
            Message::ReviewsLoaded,
        ),
    ])
}

/// Refetch whatever is on screen, if a project is picked at all.
fn reload(state: &State, client: &Client) -> Task<Message> {
    match state.project {
        Some(id) => load(client, id, &state.horizon),
        None => Task::none(),
    }
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::Refresh => {
            state.loading = true;
            Task::batch([refresh(client), reload(state, client)])
        }
        Message::ProjectsLoaded(Ok(projects)) => {
            state.error = None;
            // Open the first project on a cold start; an assistant board is
            // created server-side on first read, so there is always something.
            let first = projects.first().map(|p| p.id);
            state.projects = projects;
            match state.project {
                Some(id) if state.projects.iter().any(|p| p.id == id) => Task::none(),
                _ => match first {
                    Some(id) => {
                        state.project = Some(id);
                        state.loading = true;
                        Task::batch([
                            load(client, id, &state.horizon),
                            agenda_chat::set_project(&mut state.chat, client, Some(id))
                                .map(Message::Chat),
                        ])
                    }
                    None => {
                        state.loading = false;
                        state.dashboard = None;
                        state.project = None;
                        agenda_chat::set_project(&mut state.chat, client, None).map(Message::Chat)
                    }
                },
            }
        }
        Message::SelectProject(id) => {
            if state.project == Some(id) {
                return Task::none();
            }
            state.project = Some(id);
            state.dashboard = None;
            state.reviews.clear();
            state.loading = true;
            Task::batch([
                load(client, id, &state.horizon),
                agenda_chat::set_project(&mut state.chat, client, Some(id)).map(Message::Chat),
            ])
        }
        Message::SetHorizon(horizon) => {
            if state.horizon == horizon {
                return Task::none();
            }
            state.horizon = horizon;
            state.loading = true;
            reload(state, client)
        }
        Message::DashboardLoaded(Ok(dashboard)) => {
            state.loading = false;
            state.error = None;
            state.dashboard = Some(*dashboard);
            Task::none()
        }
        Message::ReviewsLoaded(Ok(reviews)) => {
            state.error = None;
            state.reviews = reviews;
            Task::none()
        }
        Message::Complete(item) => {
            state.busy = true;
            let c = client.clone();
            Task::perform(
                async move { err_string(c.assistant_complete_item(item).await).map(|_| ()) },
                Message::Done,
            )
        }
        Message::RunReview => {
            let Some(project) = state.project else { return Task::none() };
            state.busy = true;
            let c = client.clone();
            Task::perform(
                async move { err_string(c.assistant_run_review(project).await).map(|_| ()) },
                Message::Done,
            )
        }
        Message::ApplyReview(id) => {
            state.busy = true;
            let c = client.clone();
            Task::perform(
                async move { err_string(c.assistant_apply_review(id).await).map(|_| ()) },
                Message::Done,
            )
        }
        Message::DismissReview(id) => {
            state.busy = true;
            let c = client.clone();
            Task::perform(
                async move { err_string(c.assistant_dismiss_review(id).await).map(|_| ()) },
                Message::Done,
            )
        }
        Message::Done(Ok(())) => {
            state.busy = false;
            reload(state, client)
        }
        Message::Dismiss => {
            state.error = None;
            Task::none()
        }
        // An applied proposal writes to the board this screen is showing, so the
        // board is refetched alongside the chat's own reload — the point of
        // putting the two side by side is that the rows appear as they are
        // approved.
        Message::Chat(msg) => {
            let board_changed = matches!(msg, agenda_chat::Message::Applied(Ok(_)));
            let task = agenda_chat::update(&mut state.chat, client, msg).map(Message::Chat);
            if board_changed {
                Task::batch([task, reload(state, client)])
            } else {
                task
            }
        }

        Message::ProjectsLoaded(Err(e))
        | Message::DashboardLoaded(Err(e))
        | Message::ReviewsLoaded(Err(e))
        | Message::Done(Err(e)) => {
            state.loading = false;
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn project(id: i64, name: &str) -> ProjectSummary {
        ProjectSummary {
            id,
            name: name.into(),
            description: None,
            color: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// A cold start must land on a project, or the screen is a picker with
    /// nothing behind it.
    #[test]
    fn the_first_project_opens_itself() {
        let mut state = State::default();
        let _ = update(
            &mut state,
            &client(),
            Message::ProjectsLoaded(Ok(vec![project(7, "a"), project(9, "b")])),
        );
        assert_eq!(state.project, Some(7));
        assert!(state.loading, "the dashboard fetch is in flight");
    }

    /// Reloading the list must not yank the user off the project they picked.
    #[test]
    fn a_chosen_project_survives_a_refresh() {
        let mut state = State { project: Some(9), ..State::default() };
        let _ = update(
            &mut state,
            &client(),
            Message::ProjectsLoaded(Ok(vec![project(7, "a"), project(9, "b")])),
        );
        assert_eq!(state.project, Some(9));
    }

    /// A project that was deleted elsewhere leaves a stale selection pointing at
    /// nothing; the screen falls back rather than showing an empty pane.
    #[test]
    fn a_vanished_project_falls_back_to_the_first() {
        let mut state = State { project: Some(42), ..State::default() };
        let _ = update(&mut state, &client(), Message::ProjectsLoaded(Ok(vec![project(7, "a")])));
        assert_eq!(state.project, Some(7));
    }

    #[test]
    fn switching_horizon_clears_nothing_but_refetches() {
        let mut state = State { project: Some(1), ..State::default() };
        let _ = update(&mut state, &client(), Message::SetHorizon("week".into()));
        assert_eq!(state.horizon, "week");
        assert!(state.loading);
    }
}
