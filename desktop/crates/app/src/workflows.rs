//! User-authored workflows (n8n-lite): fixed step sequences the server runs on
//! demand, on an interval, or when an external app calls the run endpoint.
//!
//! Steps are edited as raw JSON in a text editor rather than a form: the server
//! owns the step schema and validates on save, so the app round-trips the JSON
//! verbatim and surfaces the server's error message when it is rejected.

use agent_platform_client::types::{WorkflowBody, WorkflowInfo, WorkflowRunInfo};
use agent_platform_client::Client;
use iced::widget::text_editor;
use iced::Task;
use serde_json::Value;

/// What a fresh workflow's steps look like, so the editor never opens empty.
const STEPS_TEMPLATE: &str = r#"[
  {
    "id": "fetch",
    "type": "http",
    "params": { "url": "https://example.com/api", "method": "GET" }
  }
]"#;

pub struct Editor {
    /// `None` while creating; the workflow id while editing.
    pub id: Option<i64>,
    pub name: String,
    pub description: String,
    /// Raw seconds; empty means "no schedule".
    pub interval: String,
    pub steps: text_editor::Content,
}

#[derive(Default)]
pub struct State {
    pub items: Vec<WorkflowInfo>,
    pub selected: Option<i64>,
    pub runs: Vec<WorkflowRunInfo>,
    /// Which run's per-step results are unfolded.
    pub expanded_run: Option<i64>,
    pub editor: Option<Editor>,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
    Loaded(Result<Vec<WorkflowInfo>, String>),
    Select(i64),
    RunsLoaded(Result<Vec<WorkflowRunInfo>, String>),
    ToggleRun(i64),
    RunNow(i64),
    RanNow(Result<Box<WorkflowRunInfo>, String>),
    SetEnabled(i64, bool),
    Delete(i64),
    Mutated(Result<(), String>),
    New,
    Edit(i64),
    NameChanged(String),
    DescriptionChanged(String),
    IntervalChanged(String),
    StepsEdited(text_editor::Action),
    CancelEditor,
    Save,
    Saved(Result<Box<WorkflowInfo>, String>),
    Dismiss,
}

fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

pub fn refresh(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.workflows().await).map(|r| r.workflows) },
        Message::Loaded,
    )
}

fn load_runs(client: &Client, id: i64) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.workflow_runs(id).await).map(|r| r.runs) },
        Message::RunsLoaded,
    )
}

/// The editor's contents as the API body, or the reason they cannot be sent.
/// Client-side checks stop at "is it JSON at all" — shape errors come back from
/// the server, which is the one place the schema lives.
fn editor_body(editor: &Editor) -> Result<WorkflowBody, String> {
    let name = editor.name.trim();
    if name.is_empty() {
        return Err("Name the workflow first.".to_string());
    }
    let steps: Vec<Value> = serde_json::from_str(&editor.steps.text())
        .map_err(|e| format!("Steps are not valid JSON: {e}"))?;
    let interval = editor.interval.trim();
    let interval_seconds = if interval.is_empty() {
        None
    } else {
        Some(interval.parse::<i64>().map_err(|_| "Interval must be a number of seconds.")?)
    };
    Ok(WorkflowBody {
        name: Some(name.to_string()),
        description: match editor.description.trim() {
            "" => None,
            d => Some(d.to_string()),
        },
        steps: Some(steps),
        enabled: None,
        interval_seconds,
        // Editing an interval away must clear it server-side; on create the
        // field is a no-op the API ignores.
        clear_interval: editor.id.is_some() && interval_seconds.is_none(),
    })
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::Refresh => refresh(client),
        Message::Loaded(Ok(items)) => {
            // A selected workflow that no longer exists takes its runs with it.
            if state.selected.is_some_and(|id| !items.iter().any(|w| w.id == id)) {
                state.selected = None;
                state.runs.clear();
            }
            state.items = items;
            Task::none()
        }
        Message::Select(id) => {
            if state.selected == Some(id) {
                state.selected = None;
                state.runs.clear();
                return Task::none();
            }
            state.selected = Some(id);
            state.runs.clear();
            state.expanded_run = None;
            load_runs(client, id)
        }
        Message::RunsLoaded(Ok(runs)) => {
            state.runs = runs;
            Task::none()
        }
        Message::ToggleRun(id) => {
            state.expanded_run = (state.expanded_run != Some(id)).then_some(id);
            Task::none()
        }
        Message::RunNow(id) => {
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move {
                    err_string(client.run_workflow(id, &serde_json::json!({})).await).map(Box::new)
                },
                Message::RanNow,
            )
        }
        Message::RanNow(result) => {
            state.busy = false;
            match result {
                Ok(run) => {
                    state.notice = Some(match run.status.as_str() {
                        "succeeded" => format!("Run #{} succeeded.", run.id),
                        _ => format!(
                            "Run #{} failed: {}",
                            run.id,
                            run.error.as_deref().unwrap_or("unknown error")
                        ),
                    });
                    // Fold the fresh run in ourselves when its workflow is the
                    // open one — no second fetch for data we already hold.
                    if state.selected == Some(run.workflow_id) {
                        state.runs.insert(0, *run);
                    }
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Message::SetEnabled(id, enabled) => {
            let client = client.clone();
            let body = WorkflowBody { enabled: Some(enabled), ..WorkflowBody::default() };
            Task::perform(
                async move { err_string(client.update_workflow(id, &body).await).map(|_| ()) },
                Message::Mutated,
            )
        }
        Message::Delete(id) => {
            let client = client.clone();
            Task::perform(
                async move { err_string(client.delete_workflow(id).await).map(|_| ()) },
                Message::Mutated,
            )
        }
        Message::Mutated(result) => {
            if let Err(e) = result {
                state.error = Some(e);
            }
            refresh(client)
        }
        Message::New => {
            state.editor = Some(Editor {
                id: None,
                name: String::new(),
                description: String::new(),
                interval: String::new(),
                steps: text_editor::Content::with_text(STEPS_TEMPLATE),
            });
            Task::none()
        }
        Message::Edit(id) => {
            if let Some(wf) = state.items.iter().find(|w| w.id == id) {
                let steps = serde_json::to_string_pretty(&wf.steps).unwrap_or_default();
                state.editor = Some(Editor {
                    id: Some(id),
                    name: wf.name.clone(),
                    description: wf.description.clone().unwrap_or_default(),
                    interval: wf.interval_seconds.map(|s| s.to_string()).unwrap_or_default(),
                    steps: text_editor::Content::with_text(&steps),
                });
            }
            Task::none()
        }
        Message::NameChanged(v) => {
            if let Some(ed) = &mut state.editor {
                ed.name = v;
            }
            Task::none()
        }
        Message::DescriptionChanged(v) => {
            if let Some(ed) = &mut state.editor {
                ed.description = v;
            }
            Task::none()
        }
        Message::IntervalChanged(v) => {
            if let Some(ed) = &mut state.editor {
                ed.interval = v;
            }
            Task::none()
        }
        Message::StepsEdited(action) => {
            if let Some(ed) = &mut state.editor {
                ed.steps.perform(action);
            }
            Task::none()
        }
        Message::CancelEditor => {
            state.editor = None;
            Task::none()
        }
        Message::Save => {
            let Some(editor) = &state.editor else { return Task::none() };
            let body = match editor_body(editor) {
                Ok(body) => body,
                Err(e) => {
                    state.error = Some(e);
                    return Task::none();
                }
            };
            state.busy = true;
            let id = editor.id;
            let client = client.clone();
            Task::perform(
                async move {
                    let result = match id {
                        Some(id) => client.update_workflow(id, &body).await,
                        None => client.create_workflow(&body).await,
                    };
                    err_string(result).map(Box::new)
                },
                Message::Saved,
            )
        }
        Message::Saved(result) => {
            state.busy = false;
            match result {
                Ok(wf) => {
                    state.notice = Some(format!("Saved \"{}\".", wf.name));
                    state.editor = None;
                    refresh(client)
                }
                Err(e) => {
                    // The editor stays open: the error names what to fix.
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Message::Loaded(Err(e)) | Message::RunsLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::Dismiss => {
            state.error = None;
            state.notice = None;
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(name: &str, steps: &str, interval: &str, id: Option<i64>) -> Editor {
        Editor {
            id,
            name: name.to_string(),
            description: String::new(),
            interval: interval.to_string(),
            steps: text_editor::Content::with_text(steps),
        }
    }

    #[test]
    fn editor_rejects_missing_name_and_bad_json() {
        assert!(editor_body(&editor("", "[]", "", None)).is_err());
        assert!(editor_body(&editor("wf", "not json", "", None))
            .unwrap_err()
            .contains("not valid JSON"));
        assert!(editor_body(&editor("wf", "[]", "soon", None)).is_err());
    }

    #[test]
    fn editor_builds_body_and_clears_interval_only_when_editing() {
        let body = editor_body(&editor("wf", STEPS_TEMPLATE, "300", None)).unwrap();
        assert_eq!(body.name.as_deref(), Some("wf"));
        assert_eq!(body.interval_seconds, Some(300));
        assert!(!body.clear_interval);
        assert_eq!(body.steps.as_ref().map(|s| s.len()), Some(1));

        // On create, an empty interval is simply absent…
        assert!(!editor_body(&editor("wf", "[]", "", None)).unwrap().clear_interval);
        // …but on edit it means "remove the schedule".
        assert!(editor_body(&editor("wf", "[]", "", Some(3))).unwrap().clear_interval);
    }
}
