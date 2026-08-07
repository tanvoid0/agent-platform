//! User-authored workflows (n8n-lite): fixed step sequences the server runs on
//! demand, on an interval, or when an external app calls the run endpoint.
//!
//! Steps are edited as raw JSON in a text editor rather than a form: the server
//! owns the step schema and validates on save, so the app round-trips the JSON
//! verbatim and surfaces the server's error message when it is rejected.

use agent_platform_client::types::{
    WorkflowAssistBody, WorkflowAssistResponse, WorkflowBody, WorkflowInfo, WorkflowRunInfo,
};
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
    /// The "ask AI" side of the editor: prompt in, last reply out.
    pub assist_prompt: String,
    pub assist_reply: Option<String>,
    pub assist_busy: bool,
}

impl Editor {
    fn new(id: Option<i64>, name: &str, description: &str, interval: &str, steps: &str) -> Self {
        Editor {
            id,
            name: name.to_string(),
            description: description.to_string(),
            interval: interval.to_string(),
            steps: text_editor::Content::with_text(steps),
            assist_prompt: String::new(),
            assist_reply: None,
            assist_busy: false,
        }
    }
}

#[derive(Default)]
pub struct State {
    pub items: Vec<WorkflowInfo>,
    /// False until the first list response lands, so the empty state can say
    /// "loading" instead of "you have none".
    pub loaded: bool,
    pub selected: Option<i64>,
    pub runs: Vec<WorkflowRunInfo>,
    pub runs_loading: bool,
    /// Which run's per-step results are unfolded.
    pub expanded_run: Option<i64>,
    pub editor: Option<Editor>,
    /// Which workflow is mid-run; only that card's button says "Running…".
    pub running: Option<i64>,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: crate::domain::Toast,
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
    AssistPromptChanged(String),
    AskAssist,
    AssistReplied(Result<Box<WorkflowAssistResponse>, String>),
    CancelEditor,
    Save,
    Saved(Result<Box<WorkflowInfo>, String>),
    Dismiss,
}

fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

/// Step errors can carry a whole HTML error page; the banner needs one line,
/// the full text stays in the run's step results.
fn truncate_error(e: &str) -> String {
    const MAX: usize = 220;
    if e.chars().count() <= MAX {
        e.to_string()
    } else {
        format!("{}… (full error in the run's steps)", e.chars().take(MAX).collect::<String>())
    }
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
            state.loaded = true;
            state.error = None;
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
                state.runs_loading = false;
                return Task::none();
            }
            state.selected = Some(id);
            state.runs.clear();
            state.runs_loading = true;
            state.expanded_run = None;
            load_runs(client, id)
        }
        Message::RunsLoaded(Ok(runs)) => {
            state.runs_loading = false;
            state.runs = runs;
            Task::none()
        }
        Message::ToggleRun(id) => {
            state.expanded_run = (state.expanded_run != Some(id)).then_some(id);
            Task::none()
        }
        Message::RunNow(id) => {
            if state.running.is_some() {
                return Task::none(); // one run at a time; the button already says so
            }
            state.running = Some(id);
            let client = client.clone();
            Task::perform(
                async move {
                    err_string(client.run_workflow(id, &serde_json::json!({})).await).map(Box::new)
                },
                Message::RanNow,
            )
        }
        Message::RanNow(result) => {
            state.running = None;
            match result {
                Ok(run) => {
                    // The banner and the error below are on this page; a run
                    // the user walked away from needs telling.
                    crate::notify::away(
                        "workflows",
                        &format!("Workflow run #{}", run.id),
                        &format!(
                            "{}{}",
                            run.status,
                            run.error
                                .as_deref()
                                .map(|e| format!(" — {}", truncate_error(e)))
                                .unwrap_or_default()
                        ),
                    );
                    // A failed run is bad news and must look like it; only a
                    // clean run goes in the green banner.
                    if run.status == "succeeded" {
                        state.notice.set(format!("Run #{} succeeded.", run.id));
                    } else {
                        state.error = Some(format!(
                            "Run #{} failed — {}",
                            run.id,
                            truncate_error(run.error.as_deref().unwrap_or("unknown error"))
                        ));
                    }
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
            state.editor = Some(Editor::new(None, "", "", "", STEPS_TEMPLATE));
            Task::none()
        }
        Message::Edit(id) => {
            if let Some(wf) = state.items.iter().find(|w| w.id == id) {
                let steps = serde_json::to_string_pretty(&wf.steps).unwrap_or_default();
                state.editor = Some(Editor::new(
                    Some(id),
                    &wf.name,
                    wf.description.as_deref().unwrap_or(""),
                    &wf.interval_seconds.map(|s| s.to_string()).unwrap_or_default(),
                    &steps,
                ));
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
        Message::AssistPromptChanged(v) => {
            if let Some(ed) = &mut state.editor {
                ed.assist_prompt = v;
            }
            Task::none()
        }
        Message::AskAssist => {
            let Some(ed) = &mut state.editor else { return Task::none() };
            let message = ed.assist_prompt.trim().to_string();
            if message.is_empty() || ed.assist_busy {
                return Task::none();
            }
            ed.assist_busy = true;
            // The untouched boilerplate is not a draft worth defending — sending
            // it made the model keep the example.com step in its answers.
            let steps_text = ed.steps.text();
            let is_template = steps_text.trim() == STEPS_TEMPLATE.trim();
            let body = WorkflowAssistBody {
                message,
                name: (!ed.name.trim().is_empty()).then(|| ed.name.trim().to_string()),
                // An unparseable draft is simply not sent; the model starts fresh.
                steps: (!is_template).then(|| serde_json::from_str(&steps_text).ok()).flatten(),
            };
            let client = client.clone();
            Task::perform(
                async move { err_string(client.workflow_assist(&body).await).map(Box::new) },
                Message::AssistReplied,
            )
        }
        Message::AssistReplied(result) => {
            let Some(ed) = &mut state.editor else { return Task::none() };
            ed.assist_busy = false;
            match result {
                Ok(resp) => {
                    if let Some(steps) = &resp.steps {
                        let pretty = serde_json::to_string_pretty(steps).unwrap_or_default();
                        ed.steps = text_editor::Content::with_text(&pretty);
                    }
                    ed.assist_reply = Some(resp.reply);
                    ed.assist_prompt.clear();
                }
                Err(e) => ed.assist_reply = Some(format!("Assistant error: {e}")),
            }
            Task::none()
        }
        Message::CancelEditor => {
            state.editor = None;
            Task::none()
        }
        Message::Save => {
            if state.busy {
                return Task::none(); // a save is already in flight
            }
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
                    state.notice.set(format!("Saved \"{}\".", wf.name));
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
        Message::Loaded(Err(e)) => {
            state.loaded = true;
            state.error = Some(e);
            Task::none()
        }
        Message::RunsLoaded(Err(e)) => {
            state.runs_loading = false;
            state.error = Some(e);
            Task::none()
        }
        Message::Dismiss => {
            state.error = None;
            state.notice.clear();
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(name: &str, steps: &str, interval: &str, id: Option<i64>) -> Editor {
        Editor::new(id, name, "", interval, steps)
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

    #[test]
    fn failed_run_lands_in_the_error_banner_not_the_notice() {
        let client = Client::new("http://127.0.0.1:1", "k");
        let mut s = State::default();
        let run: WorkflowRunInfo = serde_json::from_value(serde_json::json!({
            "id": 3, "workflow_id": 1, "trigger": "api", "status": "failed",
            "input": {}, "steps": [], "error": "step 'fetch': http 404",
            "started_at": "2026-08-04T00:00:00", "finished_at": null
        }))
        .unwrap();
        let _ = update(&mut s, &client, Message::RunNow(1));
        assert_eq!(s.running, Some(1));
        // a second Run click on any card is ignored while one is in flight
        let _ = update(&mut s, &client, Message::RunNow(2));
        assert_eq!(s.running, Some(1));

        let _ = update(&mut s, &client, Message::RanNow(Ok(Box::new(run))));
        assert_eq!(s.running, None);
        assert!(s.notice.is_none());
        assert!(s.error.as_deref().unwrap().contains("http 404"));

        let long = "x".repeat(1000);
        assert!(truncate_error(&long).len() < 300);
    }

    #[test]
    fn assist_reply_replaces_steps_and_clears_prompt() {
        let client = Client::new("http://127.0.0.1:1", "k");
        let mut s = State::default();
        s.editor = Some(editor("wf", "[]", "", None));
        s.editor.as_mut().unwrap().assist_prompt = "add a health check".into();

        let resp = WorkflowAssistResponse {
            reply: "Added.".into(),
            steps: Some(vec![serde_json::json!(
                {"id": "ping", "type": "http", "params": {"url": "http://x"}}
            )]),
        };
        let _ = update(&mut s, &client, Message::AssistReplied(Ok(Box::new(resp))));

        let ed = s.editor.as_ref().unwrap();
        assert!(ed.steps.text().contains("\"ping\""));
        assert_eq!(ed.assist_reply.as_deref(), Some("Added."));
        assert!(ed.assist_prompt.is_empty());

        // a review with no steps leaves the draft alone
        let before = ed.steps.text();
        let resp = WorkflowAssistResponse { reply: "Looks fine.".into(), steps: None };
        let _ = update(&mut s, &client, Message::AssistReplied(Ok(Box::new(resp))));
        assert_eq!(s.editor.as_ref().unwrap().steps.text(), before);
    }

    use agent_platform_client::Client;
}
