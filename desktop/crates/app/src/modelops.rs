//! Model ops: fine-tune projects, build jobs, the local Ollama model list and
//! the adapter registry.
//!
//! Build jobs are polled rather than streamed — the job endpoint already
//! returns `log_tail`, so one poll gives both status and logs.

use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::Task;
use std::time::Duration;

/// Pipeline stages a build job can run, in execution order.
pub const STAGES: [&str; 5] = ["prepare", "train", "merge", "quantize", "register"];

#[derive(Default)]
pub struct State {
    pub projects: Vec<ModelProject>,
    pub ollama: Vec<OllamaModelSummary>,
    pub registry: Vec<ModelRegistryEntry>,
    pub selected: Option<String>,
    /// The job being watched; polled while it is not terminal.
    pub job: Option<ModelBuildJob>,
    pub new_project: Option<NewProject>,
    pub stages: Vec<String>,
    pub register_alias: String,
    pub pull_name: String,
    pub busy: bool,
    pub uploading: bool,
    pub error: Option<String>,
    pub notice: crate::domain::Toast,
    /// Whether the first refresh has completed, so empty-state copy is
    /// only shown once we actually know the lists are empty.
    pub loaded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NewProject {
    pub name: String,
    pub description: String,
    pub base_model: String,
}

impl State {
    pub fn selected_project(&self) -> Option<&ModelProject> {
        let name = self.selected.as_deref()?;
        self.projects.iter().find(|p| p.name == name)
    }

    /// A job still moving; drives both the poll subscription and the UI badge.
    pub fn job_running(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| !matches!(j.status.as_str(), "completed" | "failed" | "cancelled"))
    }

    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(2)
    }

    /// Stages in canonical order, whatever order they were toggled in.
    pub fn ordered_stages(&self) -> Vec<String> {
        STAGES
            .iter()
            .filter(|s| self.stages.iter().any(|sel| sel == *s))
            .map(|s| s.to_string())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
    ProjectsLoaded(Result<Vec<ModelProject>, String>),
    OllamaLoaded(Result<Vec<OllamaModelSummary>, String>),
    RegistryLoaded(Result<Vec<ModelRegistryEntry>, String>),

    Select(String),
    ToggleStage(String, bool),
    AliasChanged(String),
    StartBuild,

    NewProject,
    CancelNewProject,
    NewNameChanged(String),
    NewDescriptionChanged(String),
    NewBaseModelChanged(String),
    CreateProject,

    PullNameChanged(String),
    PullModel,

    JobStarted(Result<Box<ModelBuildJob>, String>),
    JobTick,
    JobUpdated(Result<Box<ModelBuildJob>, String>),
    CloseJob,
    Done(Result<String, String>),
    DismissNotice,

    PickDatasetFile,
    DatasetUploaded(Result<Option<String>, String>),
}

fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

pub fn refresh(client: &Client) -> Task<Message> {
    let (c1, c2, c3) = (client.clone(), client.clone(), client.clone());
    Task::batch([
        Task::perform(
            async move { err_string(c1.model_projects().await).map(|r| r.projects) },
            Message::ProjectsLoaded,
        ),
        Task::perform(
            async move { err_string(c2.ollama_models().await).map(|r| r.models) },
            Message::OllamaLoaded,
        ),
        Task::perform(
            async move { err_string(c3.model_registry().await).map(|r| r.entries) },
            Message::RegistryLoaded,
        ),
    ])
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::Refresh => refresh(client),
        Message::ProjectsLoaded(Ok(projects)) => {
            if state.selected.is_none() {
                state.selected = projects.first().map(|p| p.name.clone());
            }
            state.projects = projects;
            state.loaded = true;
            Task::none()
        }
        Message::OllamaLoaded(Ok(models)) => {
            state.ollama = models;
            state.loaded = true;
            Task::none()
        }
        Message::RegistryLoaded(Ok(entries)) => {
            state.registry = entries;
            state.loaded = true;
            Task::none()
        }
        // Ollama being absent is normal on a fresh install, so it must not
        // clobber the screen with an error banner.
        Message::OllamaLoaded(Err(_)) => {
            state.loaded = true;
            Task::none()
        }
        Message::ProjectsLoaded(Err(e)) | Message::RegistryLoaded(Err(e)) => {
            state.error = Some(e);
            state.loaded = true;
            Task::none()
        }

        Message::Select(name) => {
            state.selected = Some(name);
            Task::none()
        }
        Message::ToggleStage(stage, on) => {
            state.stages.retain(|s| s != &stage);
            if on {
                state.stages.push(stage);
            }
            Task::none()
        }
        Message::AliasChanged(v) => {
            state.register_alias = v;
            Task::none()
        }
        Message::StartBuild => {
            let Some(project) = state.selected.clone() else {
                state.error = Some("Pick a model project first.".into());
                return Task::none();
            };
            let stages = state.ordered_stages();
            if stages.is_empty() {
                state.error = Some("Pick at least one stage.".into());
                return Task::none();
            }
            let body = ModelBuildJobBody {
                project,
                stages,
                register_alias: non_empty(&state.register_alias),
                offline_eval: None,
                process_id: None,
            };
            state.busy = true;
            // A build is about to want this machine's GPU. Hand back whatever
            // the in-process chat model is holding; the next turn reloads it.
            #[cfg(feature = "local-llm")]
            crate::local_llm::unload();
            let client = client.clone();
            Task::perform(
                async move { err_string(client.start_model_build_job(&body).await).map(Box::new) },
                Message::JobStarted,
            )
        }

        Message::NewProject => {
            state.new_project = Some(NewProject::default());
            Task::none()
        }
        Message::CancelNewProject => {
            state.new_project = None;
            Task::none()
        }
        Message::NewNameChanged(v) => {
            if let Some(n) = &mut state.new_project {
                n.name = v;
            }
            Task::none()
        }
        Message::NewDescriptionChanged(v) => {
            if let Some(n) = &mut state.new_project {
                n.description = v;
            }
            Task::none()
        }
        Message::NewBaseModelChanged(v) => {
            if let Some(n) = &mut state.new_project {
                n.base_model = v;
            }
            Task::none()
        }
        Message::CreateProject => {
            let Some(draft) = state.new_project.clone() else { return Task::none() };
            if draft.name.trim().is_empty() {
                state.error = Some("Name is required.".into());
                return Task::none();
            }
            let body = ModelProjectBody {
                name: draft.name.trim().to_string(),
                description: non_empty(&draft.description),
                base_model: non_empty(&draft.base_model),
                ollama_tag: None,
            };
            state.busy = true;
            state.new_project = None;
            let client = client.clone();
            Task::perform(
                async move {
                    err_string(client.create_model_project(&body).await)
                        .map(|p| format!("Created model project “{}”.", p.name))
                },
                Message::Done,
            )
        }

        Message::PullNameChanged(v) => {
            state.pull_name = v;
            Task::none()
        }
        Message::PullModel => {
            let name = state.pull_name.trim().to_string();
            if name.is_empty() {
                state.error = Some("Enter a model name to pull.".into());
                return Task::none();
            }
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move { err_string(client.pull_ollama_model(&name).await).map(Box::new) },
                Message::JobStarted,
            )
        }

        Message::JobStarted(Ok(job)) => {
            state.busy = false;
            state.job = Some(*job);
            Task::none()
        }
        Message::JobStarted(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Message::JobTick => match state.job.as_ref().map(|j| j.id) {
            Some(id) => {
                let client = client.clone();
                Task::perform(
                    async move { err_string(client.model_build_job(id).await).map(Box::new) },
                    Message::JobUpdated,
                )
            }
            None => Task::none(),
        },
        Message::JobUpdated(Ok(job)) => {
            let previous = state.job.as_ref().map(|j| j.status.clone());
            let finished = became_terminal(previous.as_deref(), &job.status);
            if finished {
                let label = job.project_name.clone().unwrap_or_else(|| job.job_type.clone());
                crate::notify::job_finished(&format!("Build job #{}", job.id), &label, &job.status);
            }
            state.job = Some(*job);
            // A finished job changes the registry and Ollama lists.
            if finished {
                return refresh(client);
            }
            Task::none()
        }
        Message::JobUpdated(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::CloseJob => {
            state.job = None;
            Task::none()
        }

        Message::PickDatasetFile => {
            let Some(project) = state.selected.clone() else {
                state.error = Some("Pick a model project first.".into());
                return Task::none();
            };
            state.uploading = true;
            let client = client.clone();
            Task::perform(pick_and_upload_dataset(client, project), Message::DatasetUploaded)
        }
        Message::DatasetUploaded(result) => {
            state.uploading = false;
            match result {
                Ok(Some(msg)) => {
                    state.notice.set(msg);
                    refresh(client)
                }
                Ok(None) => Task::none(),
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Message::Done(result) => {
            state.busy = false;
            match result {
                Ok(msg) => {
                    state.notice.set(msg);
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

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn is_terminal_job_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

/// True only on the poll where the job first reaches a terminal status, so
/// re-polling an already-finished job does not re-fire the notification.
fn became_terminal(previous: Option<&str>, current: &str) -> bool {
    let was_terminal = previous.is_some_and(is_terminal_job_status);
    !was_terminal && is_terminal_job_status(current)
}

/// Opens a native file picker, then uploads the chosen file into the project's
/// workspace under `datasets/<filename>`. `Ok(None)` means the user cancelled.
async fn pick_and_upload_dataset(client: Client, project: String) -> Result<Option<String>, String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_title("Pick a dataset file")
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let filename = handle.file_name();
    let rel_path = format!("datasets/{filename}");
    let bytes = handle.read().await;
    client
        .upload_project_file(&project, &rel_path, bytes)
        .await
        .map(|r| Some(format!("Uploaded {rel_path} to “{project}” ({} file(s)).", r.uploaded)))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn job(status: &str) -> ModelBuildJob {
        serde_json::from_value(json!({
            "id": 1, "job_type": "build", "stages": ["train"], "status": status,
            "result": {}, "poll_url": "/p", "stream_url": "/s",
            "created_at": "2026-08-02T10:00:00",
        }))
        .unwrap()
    }

    #[test]
    fn stages_are_sent_in_pipeline_order() {
        let mut s = State::default();
        for stage in ["register", "train", "prepare"] {
            let _ = update(&mut s, &client(), Message::ToggleStage(stage.into(), true));
        }
        assert_eq!(s.ordered_stages(), vec!["prepare", "train", "register"]);
    }

    #[test]
    fn toggling_a_stage_off_removes_it_once() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::ToggleStage("train".into(), true));
        let _ = update(&mut s, &client(), Message::ToggleStage("train".into(), true));
        let _ = update(&mut s, &client(), Message::ToggleStage("train".into(), false));
        assert!(s.ordered_stages().is_empty());
    }

    #[test]
    fn build_requires_a_project_and_a_stage() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::StartBuild);
        assert!(s.error.as_deref().unwrap().contains("project"));

        s.selected = Some("p".into());
        s.error = None;
        let _ = update(&mut s, &client(), Message::StartBuild);
        assert!(s.error.as_deref().unwrap().contains("stage"));
        assert!(!s.busy);
    }

    #[test]
    fn only_a_live_job_keeps_polling() {
        let mut s = State::default();
        assert!(!s.job_running());
        s.job = Some(job("running"));
        assert!(s.job_running());
        for done in ["completed", "failed", "cancelled"] {
            s.job = Some(job(done));
            assert!(!s.job_running(), "{done}");
        }
    }

    #[test]
    fn a_missing_ollama_does_not_raise_an_error_banner() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::OllamaLoaded(Err("connection refused".into())));
        assert!(s.error.is_none());
    }

    #[test]
    fn upload_requires_a_selected_project() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::PickDatasetFile);
        assert!(s.error.as_deref().unwrap().contains("project"));
        assert!(!s.uploading);
    }

    #[test]
    fn notification_fires_once_on_the_terminal_transition() {
        assert!(!became_terminal(None, "running"));
        assert!(became_terminal(None, "completed"));
        assert!(became_terminal(Some("running"), "failed"));
        assert!(!became_terminal(Some("completed"), "completed"));
        assert!(!became_terminal(Some("failed"), "failed"));
    }
}
