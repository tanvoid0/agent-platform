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
    /// Non-2xx response; message extracted from the `{"detail": ...}` body when present.
    Api { status: u16, message: String, body: Option<Value> },
    /// 2xx response whose body did not match the expected shape.
    Decode { message: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Http(e) => write!(f, "{e}"),
            Error::Api { status, message, .. } => write!(f, "HTTP {status}: {message}"),
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

fn detail_message(body: &Value) -> String {
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
        let text = resp.text().await?;
        if !status.is_success() {
            let body: Option<Value> = serde_json::from_str(&text).ok();
            let message = body.as_ref().map(detail_message).unwrap_or_else(|| {
                if text.is_empty() { "Request failed".to_string() } else { text.clone() }
            });
            return Err(Error::Api { status: status.as_u16(), message, body });
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

    pub async fn process_events(
        &self,
        id: i64,
        event_type: Option<&str>,
        limit: u32,
    ) -> Result<ProcessEventsResponse> {
        let mut path = format!("/api/v1/processes/{id}/events?limit={limit}");
        if let Some(t) = event_type {
            path.push_str(&format!("&event_type={t}"));
        }
        self.get_json(&path).await
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

    // -- Chat ----------------------------------------------------------------

    /// Returns the assistant message content; falls back to the raw body string
    /// when the response is not OpenAI-shaped (mirrors the web client).
    pub async fn chat(&self, body: &ChatCompletionBody) -> Result<String> {
        let raw: Value = self.post_json("/api/v1/chat", body).await?;
        Ok(raw
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| raw.to_string()))
    }

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

    #[test]
    fn urlencode_path_segment() {
        assert_eq!(urlencode("my-model_1.0"), "my-model_1.0");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }
}
