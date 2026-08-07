//! Typed HTTP client for the agent-platform API. Targets `/api/v1/*` exclusively
//! (the bare-root duplicate mounts are legacy and slated for deletion).

use crate::types::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug)]
pub enum Error {
    /// Transport-level failure (connect, timeout, body read).
    Http(reqwest::Error),
    /// Non-2xx response; message extracted from the `{"detail": ...}` body when
    /// present. `trace` is the `x-request-id` the response carried (see
    /// `desktop/crates/server/src/request_id.rs`) — both servers log every
    /// request under it, so it is what the Logs screen's trace filter needs.
    Api { status: u16, message: String, body: Option<Value>, trace: Option<String> },
    /// 2xx response whose body did not match the expected shape.
    Decode { message: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // These strings are shown to the user verbatim — every screen's
            // error banner is `e.to_string()`. reqwest's own Display for a
            // refused connection is "error sending request for url (…)", which
            // says nothing about the one thing that happened: the server is not
            // up. The rest keep reqwest's wording, which is specific enough.
            Error::Http(e) if e.is_timeout() => {
                write!(f, "The server did not answer in time")
            }
            Error::Http(e) if e.is_connect() => match e.url() {
                Some(url) => {
                    write!(f, "Cannot reach the server at {}", url.origin().ascii_serialization())
                }
                None => write!(f, "Cannot reach the server"),
            },
            Error::Http(e) => write!(f, "{e}"),
            Error::Api { status, message, trace, .. } => {
                write!(f, "HTTP {status}: {message}")?;
                if let Some(id) = trace {
                    // `ui::alert_error_traced` parses this suffix back off to
                    // offer a "View logs" button — keep the two in sync.
                    write!(f, " · trace {id}")?;
                }
                Ok(())
            }
            Error::Decode { message } => write!(f, "decode error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The `x-request-id` a response carried — both servers set it on every
/// response, matched or proxied, per `request_id.rs`.
fn trace_id(resp: &reqwest::Response) -> Option<String> {
    resp.headers().get("x-request-id").and_then(|v| v.to_str().ok()).map(str::to_string)
}

pub(crate) fn detail_message(body: &Value) -> String {
    match body.get("detail") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("; "),
        _ => "Request failed".to_string(),
    }
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl Client {
    /// `base` is the server origin, e.g. `http://127.0.0.1:18410`.
    pub fn new(base: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: base.into().trim_end_matches('/').to_string(),
            key: key.into(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.bearer_auth(&self.key)
    }

    async fn handle<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        let trace = trace_id(&resp);
        let text = resp.text().await?;
        if !status.is_success() {
            let body: Option<Value> = serde_json::from_str(&text).ok();
            let message = body.as_ref().map(detail_message).unwrap_or_else(|| {
                if text.is_empty() { "Request failed".to_string() } else { text.clone() }
            });
            return Err(Error::Api { status: status.as_u16(), message, body, trace });
        }
        let text = if text.is_empty() { "{}" } else { &text };
        serde_json::from_str(text).map_err(|e| Error::Decode { message: e.to_string() })
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self.authed(self.http.get(self.url(path))).send().await?;
        Self::handle(resp).await
    }

    async fn post_json<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let resp = self.authed(self.http.post(self.url(path)).json(body)).send().await?;
        Self::handle(resp).await
    }

    async fn patch_json<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let resp = self.authed(self.http.patch(self.url(path)).json(body)).send().await?;
        Self::handle(resp).await
    }

    async fn put_json<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let resp = self.authed(self.http.put(self.url(path)).json(body)).send().await?;
        Self::handle(resp).await
    }

    async fn delete_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self.authed(self.http.delete(self.url(path))).send().await?;
        Self::handle(resp).await
    }

    // -- Processes ----------------------------------------------------------

    pub async fn processes(&self, limit: u32, filter: ProcessListFilter) -> Result<ProcessesListResponse> {
        let scope = match filter {
            ProcessListFilter::Unassigned => "unassigned_only=true".to_string(),
            ProcessListFilter::Project(id) => format!("project_id={id}"),
        };
        self.get_json(&format!("/api/v1/processes?limit={limit}&{scope}")).await
    }

    pub async fn process_detail(&self, id: i64) -> Result<ProcessDetailResponse> {
        self.get_json(&format!("/api/v1/processes/{id}")).await
    }

    /// `after_id` is the append-ordered cursor the server pages on: 0 starts at
    /// the beginning, otherwise pass the last id you already hold.
    pub async fn process_events(
        &self,
        id: i64,
        event_type: Option<&str>,
        limit: u32,
        after_id: i64,
    ) -> Result<ProcessEventsResponse> {
        let mut path = format!("/api/v1/processes/{id}/events?limit={limit}&after_id={after_id}");
        if let Some(t) = event_type {
            path.push_str(&format!("&event_type={t}"));
        }
        self.get_json(&path).await
    }

    /// Every event for a process, walking the server's `after_id` cursor. The
    /// page bound is a safety net, not a limit anyone should hit: a server that
    /// stopped advancing the cursor would otherwise loop forever.
    pub async fn all_process_events(&self, id: i64) -> Result<Vec<EventLogRecord>> {
        const PAGE: u32 = 2000;
        const MAX_PAGES: usize = 500;
        let mut out: Vec<EventLogRecord> = Vec::new();
        for _ in 0..MAX_PAGES {
            let after = out.last().map(|e| e.id).unwrap_or(0);
            let page = self.process_events(id, None, PAGE, after).await?.events;
            let short = page.len() < PAGE as usize;
            out.extend(page);
            if short {
                break;
            }
        }
        Ok(out)
    }

    pub async fn create_process(&self, body: &CreateProcessBody) -> Result<CreateProcessResponse> {
        self.post_json("/api/v1/processes", body).await
    }

    /// `dag_json` is the stringified DAG; validate with [`crate::dag::validate_planner_dag`] first.
    pub async fn approve_process(&self, id: i64, dag_json: &str) -> Result<ApproveDagResponse> {
        self.post_json(
            &format!("/api/v1/processes/{id}/approve"),
            &serde_json::json!({ "dag_json": dag_json }),
        )
        .await
    }

    pub async fn cancel_process(&self, id: i64) -> Result<CancelProcessResponse> {
        self.post_json(&format!("/api/v1/processes/{id}/cancel"), &serde_json::json!({})).await
    }

    pub async fn retry_process(&self, id: i64) -> Result<RetryProcessResponse> {
        self.post_json(&format!("/api/v1/processes/{id}/retry"), &serde_json::json!({})).await
    }

    pub async fn sync_process(&self, id: i64) -> Result<SyncProcessResponse> {
        self.post_json(&format!("/api/v1/processes/{id}/sync"), &serde_json::json!({})).await
    }

    pub async fn retry_task(&self, process_id: i64, task_id: i64) -> Result<RetryTaskResponse> {
        self.post_json(
            &format!("/api/v1/processes/{process_id}/tasks/{task_id}/retry"),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn review_task(
        &self,
        process_id: i64,
        task_id: i64,
        body: &ReviewTaskBody,
    ) -> Result<ReviewTaskResponse> {
        self.post_json(&format!("/api/v1/processes/{process_id}/tasks/{task_id}/review"), body).await
    }

    // -- Teams (trailing slash matters on the collection routes) ------------

    pub async fn teams(&self) -> Result<TeamsListResponse> {
        self.get_json("/api/v1/teams/").await
    }

    pub async fn team_detail(&self, id: i64) -> Result<TeamTemplateDetail> {
        self.get_json(&format!("/api/v1/teams/{id}")).await
    }

    pub async fn create_team(&self, body: &TeamTemplateBody) -> Result<TeamTemplateDetail> {
        self.post_json("/api/v1/teams/", body).await
    }

    pub async fn update_team(&self, id: i64, body: &TeamTemplateBody) -> Result<TeamTemplateDetail> {
        self.patch_json(&format!("/api/v1/teams/{id}"), body).await
    }

    pub async fn delete_team(&self, id: i64) -> Result<Value> {
        self.delete_json(&format!("/api/v1/teams/{id}")).await
    }

    // -- Projects ------------------------------------------------------------

    pub async fn projects(&self) -> Result<ProjectsListResponse> {
        self.get_json("/api/v1/projects/").await
    }

    pub async fn project_detail(&self, id: i64) -> Result<ProjectSummary> {
        self.get_json(&format!("/api/v1/projects/{id}")).await
    }

    pub async fn create_project(&self, body: &ProjectBody) -> Result<ProjectSummary> {
        self.post_json("/api/v1/projects/", body).await
    }

    pub async fn update_project(&self, id: i64, body: &ProjectBody) -> Result<ProjectSummary> {
        self.patch_json(&format!("/api/v1/projects/{id}"), body).await
    }

    pub async fn delete_project(&self, id: i64) -> Result<Value> {
        self.delete_json(&format!("/api/v1/projects/{id}")).await
    }

    // Chat lives in `sse::chat_stream` — every caller wants the reply as it
    // arrives, so there is no buffered variant.

    // -- System --------------------------------------------------------------

    pub async fn system_status(&self) -> Result<SystemStatus> {
        self.get_json("/api/v1/system/status").await
    }

    /// Poll with the previous response's `next` to get only new lines.
    pub async fn system_logs(&self, after: i64) -> Result<LogChunk> {
        self.get_json(&format!("/api/v1/system/logs?after={after}")).await
    }

    /// Unauthenticated readiness probe.
    pub async fn health(&self) -> Result<Value> {
        let resp = self.http.get(self.url("/health")).send().await?;
        Self::handle(resp).await
    }

    /// The server's own OpenAPI document — the whole REST surface, including the
    /// routes `agent-platformd` answers itself: Python still declares them, and
    /// the daemon proxies this path to it.
    pub async fn openapi(&self) -> Result<Value> {
        self.get_json("/openapi.json").await
    }

    // -- Model-ops ------------------------------------------------------------

    pub async fn model_projects(&self) -> Result<ModelProjectsResponse> {
        self.get_json("/api/v1/model-ops/projects").await
    }

    pub async fn create_model_project(&self, body: &ModelProjectBody) -> Result<ModelProject> {
        self.post_json("/api/v1/model-ops/projects", body).await
    }

    pub async fn model_project(&self, name: &str) -> Result<ModelProject> {
        self.get_json(&format!("/api/v1/model-ops/projects/{}", urlencode(name))).await
    }

    pub async fn start_model_build_job(&self, body: &ModelBuildJobBody) -> Result<ModelBuildJob> {
        self.post_json("/api/v1/model-ops/jobs", body).await
    }

    pub async fn model_build_job(&self, job_id: i64) -> Result<ModelBuildJob> {
        self.get_json(&format!("/api/v1/model-ops/jobs/{job_id}")).await
    }

    pub async fn ollama_models(&self) -> Result<OllamaModelsResponse> {
        self.get_json("/api/v1/model-ops/ollama/models").await
    }

    pub async fn pull_ollama_model(&self, name: &str) -> Result<ModelBuildJob> {
        self.post_json(
            "/api/v1/model-ops/ollama/models/pull",
            &serde_json::json!({ "name": name, "async": true }),
        )
        .await
    }

    pub async fn model_registry(&self) -> Result<ModelRegistryResponse> {
        self.get_json("/api/v1/model-ops/registry").await
    }

    /// Multipart upload into the project workspace. `rel_path` becomes the
    /// destination relative to the project directory (e.g. `datasets/train.jsonl`);
    /// the server derives it from the part's filename, so `rel_path` is sent as
    /// the filename rather than as a separate field.
    pub async fn upload_project_file(
        &self,
        project: &str,
        rel_path: &str,
        bytes: Vec<u8>,
    ) -> Result<UploadFilesResponse> {
        let part = reqwest::multipart::Part::bytes(bytes).file_name(rel_path.to_string());
        let form = reqwest::multipart::Form::new().part("files", part);
        let resp = self
            .authed(
                self.http
                    .post(self.url(&format!("/api/v1/model-ops/projects/{}/files", urlencode(project))))
                    .multipart(form),
            )
            .send()
            .await?;
        Self::handle(resp).await
    }

    /// Synthesize speech through the server's configured backend, returning the
    /// audio bytes. The body is audio, not JSON, so this bypasses `handle`.
    ///
    /// A 501 means no speech backend is configured — the caller is expected to
    /// fall back to a local engine rather than treat it as a failure.
    pub async fn speech(&self, text: &str) -> Result<Vec<u8>> {
        let resp = self
            .authed(
                self.http
                    .post(self.url("/v1/audio/speech"))
                    .json(&serde_json::json!({ "input": text })),
            )
            .send()
            .await?;
        let status = resp.status();
        let trace = trace_id(&resp);
        if !status.is_success() {
            let text = resp.text().await?;
            let body: Option<Value> = serde_json::from_str(&text).ok();
            let message = body.as_ref().map(detail_message).unwrap_or(text);
            return Err(Error::Api { status: status.as_u16(), message, body, trace });
        }
        Ok(resp.bytes().await?.to_vec())
    }

    // -- Todos -----------------------------------------------------------------

    pub async fn todo_boards(&self) -> Result<TodoBoardsResponse> {
        self.get_json("/api/v1/todos/boards").await
    }

    /// The whole board — categories and items in one response.
    pub async fn todo_board(&self, id: i64) -> Result<TodoBoardDetail> {
        self.get_json(&format!("/api/v1/todos/boards/{id}")).await
    }

    pub async fn create_todo_board(&self, body: &TodoBoardBody) -> Result<TodoBoardSummary> {
        self.post_json("/api/v1/todos/boards", body).await
    }

    pub async fn delete_todo_board(&self, id: i64) -> Result<()> {
        self.delete_json::<serde::de::IgnoredAny>(&format!("/api/v1/todos/boards/{id}"))
            .await
            .map(|_| ())
    }

    pub async fn create_todo_item(&self, board: i64, body: &TodoItemBody) -> Result<TodoItem> {
        self.post_json(&format!("/api/v1/todos/boards/{board}/items"), body).await
    }

    pub async fn update_todo_item(&self, id: i64, body: &TodoItemPatch) -> Result<TodoItem> {
        self.patch_json(&format!("/api/v1/todos/items/{id}"), body).await
    }

    pub async fn delete_todo_item(&self, id: i64) -> Result<()> {
        self.delete_json::<serde::de::IgnoredAny>(&format!("/api/v1/todos/items/{id}"))
            .await
            .map(|_| ())
    }

    // -- Coder agent -----------------------------------------------------------

    /// Open a coder session. The id is needed *before* the first turn streams:
    /// answering a delegated tool call is addressed by `(thread_id, call_id)`,
    /// and the id would otherwise only arrive with the turn that already needs it.
    pub async fn create_coder_thread(
        &self,
        workspace_root: &str,
    ) -> Result<CoderThreadCreateOut> {
        self.post_json(
            "/api/v1/coder/chat/threads",
            &serde_json::json!({ "workspace_root": workspace_root }),
        )
        .await
    }

    pub async fn coder_threads(&self) -> Result<CoderThreadsListOut> {
        self.get_json("/api/v1/coder/chat/threads").await
    }

    /// One thread with its full history, for reopening a past session.
    pub async fn coder_thread(&self, id: i64) -> Result<CoderThreadOut> {
        self.get_json(&format!("/api/v1/coder/chat/thread?thread_id={id}")).await
    }

    pub async fn delete_coder_thread(&self, id: i64) -> Result<()> {
        self.delete_json::<serde::de::IgnoredAny>(&format!("/api/v1/coder/chat/thread/{id}"))
            .await
            .map(|_| ())
    }

    /// Hand back what a delegated tool call produced on this machine. The agent
    /// turn is parked on this until it lands.
    pub async fn coder_tool_result(&self, thread_id: i64, call_id: &str, result: &str) -> Result<()> {
        self.post_json::<serde::de::IgnoredAny>(
            "/api/v1/coder/chat/tool-result",
            &serde_json::json!({
                "thread_id": thread_id,
                "call_id": call_id,
                "result": result,
            }),
        )
        .await
        .map(|_| ())
    }

    // -- Personal assistant ----------------------------------------------------

    /// The assistant's board for one project, sliced by horizon (`day`, `week`,
    /// `month`). The board is created on first read, server-side.
    pub async fn assistant_dashboard(
        &self,
        project: i64,
        horizon: &str,
    ) -> Result<AssistantDashboard> {
        self.get_json(&format!(
            "/api/v1/assistant/dashboard?project_id={project}&horizon={}",
            urlencode(horizon)
        ))
        .await
    }

    /// Log a completion. The item is addressed by bare id — the server resolves
    /// the project itself — so no `project_id` goes on this one.
    pub async fn assistant_complete_item(&self, item: i64) -> Result<TodoItem> {
        self.post_json(&format!("/api/v1/assistant/items/{item}/complete"), &serde_json::json!({}))
            .await
    }

    /// Runs the reviewer against the board — an LLM call, so it is slow.
    pub async fn assistant_run_review(&self, project: i64) -> Result<AssistantReview> {
        self.post_json(
            &format!("/api/v1/assistant/reviews/run?project_id={project}"),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn assistant_pending_reviews(&self, project: i64) -> Result<AssistantReviewsResponse> {
        self.get_json(&format!("/api/v1/assistant/reviews/pending?project_id={project}"))
            .await
    }

    /// Applies every action the review proposed; an empty body means "all of
    /// them", which is the only thing this screen offers.
    pub async fn assistant_apply_review(&self, review: i64) -> Result<Value> {
        self.post_json(&format!("/api/v1/assistant/reviews/{review}/apply"), &serde_json::json!({}))
            .await
    }

    pub async fn assistant_dismiss_review(&self, review: i64) -> Result<Value> {
        self.post_json(&format!("/api/v1/assistant/reviews/{review}/dismiss"), &serde_json::json!({}))
            .await
    }

    // -- Personal assistant: the planning chat ---------------------------------

    pub async fn assistant_threads(&self, project: i64) -> Result<AssistantThreadsResponse> {
        self.get_json(&format!("/api/v1/assistant/chat/threads?project_id={project}")).await
    }

    /// A fresh thread. Without this, `assistant_thread(project, None)` opens the
    /// most recently touched one — there is no "send into a new thread" flag.
    pub async fn assistant_new_thread(&self, project: i64) -> Result<AssistantThreadCreated> {
        self.post_json(
            &format!("/api/v1/assistant/chat/threads?project_id={project}"),
            &serde_json::json!({}),
        )
        .await
    }

    /// `None` opens the most recently updated thread, creating one if the project
    /// has never been chatted to.
    pub async fn assistant_thread(
        &self,
        project: i64,
        thread: Option<i64>,
    ) -> Result<AssistantChatThread> {
        let mut path = format!("/api/v1/assistant/chat/thread?project_id={project}");
        if let Some(id) = thread {
            path.push_str(&format!("&thread_id={id}"));
        }
        self.get_json(&path).await
    }

    /// One LLM turn: routes to a domain profile, plans board actions, replies.
    /// Slow — minutes on a local model.
    pub async fn assistant_chat_send(
        &self,
        project: i64,
        body: &AssistantChatSend,
    ) -> Result<AssistantChatThread> {
        self.post_json(&format!("/api/v1/assistant/chat/send?project_id={project}"), body).await
    }

    /// Drops everything after `message_index` and regenerates from that user turn.
    pub async fn assistant_chat_retry(
        &self,
        project: i64,
        body: &AssistantChatRetry,
    ) -> Result<AssistantChatThread> {
        self.post_json(&format!("/api/v1/assistant/chat/retry?project_id={project}"), body).await
    }

    /// Saves the answers (to the domain profile, or as a chat turn for a
    /// clarifying form) and continues the conversation from them.
    pub async fn assistant_submit_form(
        &self,
        project: i64,
        body: &AssistantFormSubmit,
    ) -> Result<AssistantChatThread> {
        self.post_json(&format!("/api/v1/assistant/chat/submit-form?project_id={project}"), body)
            .await
    }

    /// Applies proposed actions to the assistant's board. An empty `actions`
    /// dismisses the proposal instead.
    pub async fn assistant_apply_actions(
        &self,
        project: i64,
        body: &AssistantApplyBody,
    ) -> Result<AssistantApplyResult> {
        self.post_json(&format!("/api/v1/assistant/chat/apply?project_id={project}"), body).await
    }

    // -- LLM providers ---------------------------------------------------------

    pub async fn llm_env(&self) -> Result<LlmEnv> {
        self.get_json("/api/v1/llm-proxy/env").await
    }

    pub async fn save_llm_env(&self, body: &EnvUpdate) -> Result<EnvSaveResponse> {
        self.post_json("/api/v1/llm-proxy/env", body).await
    }

    pub async fn llm_providers(&self) -> Result<ProviderCatalog> {
        self.get_json("/api/v1/llm-proxy/ui/providers").await
    }

    // -- Workflows -------------------------------------------------------------

    pub async fn workflows(&self) -> Result<WorkflowsListResponse> {
        self.get_json("/api/v1/workflows").await
    }

    pub async fn create_workflow(&self, body: &WorkflowBody) -> Result<WorkflowInfo> {
        self.post_json("/api/v1/workflows", body).await
    }

    pub async fn update_workflow(&self, id: i64, body: &WorkflowBody) -> Result<WorkflowInfo> {
        self.put_json(&format!("/api/v1/workflows/{id}"), body).await
    }

    pub async fn delete_workflow(&self, id: i64) -> Result<Value> {
        self.delete_json(&format!("/api/v1/workflows/{id}")).await
    }

    /// Runs synchronously server-side; the response is the finished run.
    pub async fn run_workflow(&self, id: i64, input: &Value) -> Result<WorkflowRunInfo> {
        self.post_json(&format!("/api/v1/workflows/{id}/run"), input).await
    }

    pub async fn workflow_runs(&self, id: i64) -> Result<WorkflowRunsResponse> {
        self.get_json(&format!("/api/v1/workflows/{id}/runs?limit=20")).await
    }

    /// Chat-style generate/review/edit of a workflow's steps.
    pub async fn workflow_assist(&self, body: &WorkflowAssistBody) -> Result<WorkflowAssistResponse> {
        self.post_json("/api/v1/workflows/assist", body).await
    }
}

/// Minimal percent-encoding for path segments (model project names).
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_string_and_array() {
        assert_eq!(detail_message(&serde_json::json!({"detail": "boom"})), "boom");
        assert_eq!(
            detail_message(&serde_json::json!({"detail": ["a", "b"]})),
            "a; b"
        );
        assert_eq!(detail_message(&serde_json::json!({})), "Request failed");
    }

    /// Every screen renders `Error::to_string()` straight into its banner, so
    /// this string is user copy. A dead port is the case the user actually hits
    /// (the app races the daemon's startup), and reqwest's own wording for it —
    /// "error sending request for url (…)" — describes the library, not the
    /// problem.
    #[tokio::test]
    async fn a_refused_connection_reads_as_a_server_that_is_not_up() {
        // Port 1 is privileged and unbound: connect fails without a timeout wait.
        let err = Client::new("http://127.0.0.1:1", "k")
            .projects()
            .await
            .expect_err("nothing is listening on port 1");
        let message = err.to_string();
        assert!(
            message.starts_with("Cannot reach the server at http://127.0.0.1:1"),
            "unhelpful transport error reached the user: {message}"
        );
        assert!(!message.contains("error sending request"));
    }

    #[test]
    fn urlencode_path_segment() {
        assert_eq!(urlencode("my-model_1.0"), "my-model_1.0");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }

    /// Verbatim `DashboardOut`, dumped from the server's own pydantic model —
    /// the assistant's items are the fat `ItemOut`, and [`TodoItem`] is a
    /// subset of it. A field renamed server-side breaks here, not on screen.
    #[test]
    fn a_dashboard_payload_decodes() {
        // `##` delimiters: the category color is `"#fff"`, which would close a
        // plain `r#"…"#`.
        let body = r##"{"project_id":1,"board_id":2,"horizon":"day","range_start":"x","range_end":"y","categories":[{"id":3,"board_id":2,"name":"c","color":"#fff","sort_order":0,"planner_profile_id":null,"created_at":"2026-08-05T21:44:25","updated_at":"2026-08-05T21:44:25"}],"items":[{"id":1,"board_id":2,"category_id":3,"title":"t","description":"d","status":"plan","priority":1,"tags":["a"],"plan":[],"metadata":{},"assigned_profile_id":null,"linked_process_id":null,"parent_item_id":null,"due_at":"2026-08-06T00:00:00","scheduled_at":null,"time_horizon":null,"item_kind":null,"recurrence":{},"completion":{},"created_at":"2026-08-05T21:44:25","updated_at":"2026-08-05T21:44:25"}],"subtasks_by_parent":{},"overdue":[],"habits_due":[],"goals":[],"stats":{"total_items":1,"done_count":0,"active_count":1,"overdue_count":1,"habits_due_count":0}}"##;
        let d: AssistantDashboard = serde_json::from_str(body).unwrap();
        assert_eq!(d.horizon, "day");
        assert_eq!(d.items[0].title, "t");
        assert_eq!(d.categories[0].name, "c");
        assert_eq!(d.stats.overdue_count, 1);
    }

    /// `POST /reviews/run` keys the id as `review_id`; the pending list keys it
    /// as `id`. One type reads both, so the banner works either way.
    #[test]
    fn a_review_decodes_under_both_id_names() {
        let run = r#"{"review_id":5,"status":"pending","summary":"s","stats":{},"proposed_actions":[{"action_id":"a1","name":"create_item","parameters":{},"confidence":0.9,"reasoning":"why"}]}"#;
        let pending = r#"{"reviews":[{"id":5,"status":"pending","summary":null,"stats":{},"proposed_actions":[],"created_at":"2026-08-05T21:44:25"}]}"#;
        let run: AssistantReview = serde_json::from_str(run).unwrap();
        assert_eq!(run.id, 5);
        assert_eq!(run.proposed_actions[0].reasoning.as_deref(), Some("why"));
        let list: AssistantReviewsResponse = serde_json::from_str(pending).unwrap();
        assert_eq!(list.reviews[0].id, 5);
    }
}
