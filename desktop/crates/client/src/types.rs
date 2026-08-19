//! Contracts for agent-platform FastAPI JSON responses.
//! Mirrors `web/src/api/types.ts` / `web/src/api/system.ts` / `web/src/api/modelOps.ts`,
//! which in turn track `app/models.py` and route payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub use crate::enums::*;

// ---------------------------------------------------------------------------
// Planner DAG (mirrors app/dag_schema.py)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentNode {
    pub client_uuid: String,
    pub role: String,
    pub system_prompt: String,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    /// LLM proxy chat `model` alias; omit for server/env defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When true, backend may append child tasks after this node completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdecompose: Option<bool>,
    /// When true, task pauses for review after LLM output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_review: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerDag {
    pub team_name: String,
    pub goal_restatement: String,
    pub subagents: Vec<SubagentNode>,
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub id: i64,
    pub goal: String,
    pub status: ProcessStatus,
    pub dag_json: Option<String>,
    pub failure_reason: Option<String>,
    pub total_tokens: i64,
    pub total_cost: f64,
    #[serde(default)]
    pub tool_invocations_used: Option<i64>,
    #[serde(default)]
    pub team_template_id: Option<i64>,
    #[serde(default)]
    pub team_snapshot_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub project_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNodeRecord {
    pub id: i64,
    pub process_id: i64,
    pub client_uuid: String,
    #[serde(default)]
    pub parent_client_uuid: Option<String>,
    pub role: String,
    pub system_prompt: String,
    pub instructions: String,
    pub llm_model: Option<String>,
    pub dependencies_json: String,
    pub status: String,
    #[serde(default)]
    pub requires_review: Option<bool>,
    #[serde(default)]
    pub reviewer_client_uuid: Option<String>,
    #[serde(default)]
    pub review_feedback: Option<String>,
    #[serde(default)]
    pub revision_count: Option<i64>,
    #[serde(default)]
    pub draft_output: Option<String>,
    /// Server JSON string with exception details when status is failed.
    #[serde(default)]
    pub failure_debug_json: Option<String>,
    pub output: Option<String>,
    pub tokens_used: i64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessesListResponse {
    pub processes: Vec<ProcessRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessDetailResponse {
    pub process: ProcessRecord,
    pub tasks: Vec<TaskNodeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLogRecord {
    pub id: i64,
    pub process_id: i64,
    pub task_id: Option<i64>,
    pub event_type: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessEventsResponse {
    pub events: Vec<EventLogRecord>,
}

/// `GET /processes` requires an explicit scope and 400s without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessListFilter {
    Unassigned,
    Project(i64),
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateProcessBody {
    pub goal: String,
    pub team_template_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_approve: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProcessResponse {
    pub process_id: i64,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApproveDagResponse {
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub idempotent: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CancelProcessResponse {
    pub status: String,
    #[serde(default)]
    pub idempotent: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetryProcessResponse {
    pub process_id: i64,
    pub status: String,
    pub retry: ProcessRetryMode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncProcessResponse {
    pub process_id: i64,
    pub process_status: String,
    pub action: ProcessSyncAction,
    pub detail: String,
    #[serde(default)]
    pub task_counts: Option<HashMap<String, i64>>,
    #[serde(default)]
    pub reset_running_tasks: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetryTaskResponse {
    pub process_id: i64,
    pub task_id: i64,
    pub status: String,
    pub retry: ProcessRetryMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewTaskBody {
    pub decision: ReviewDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewTaskResponse {
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub revision_count: Option<i64>,
    #[serde(default)]
    pub idempotent: Option<bool>,
}

// ---------------------------------------------------------------------------
// Teams (mirrors app/team_schema.py roster JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RosterRole {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Declared output modality ("text" | "audio" | "video" | "image"); defaults to text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Optional #hex for map chrome; planner ignores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamRoster {
    pub roles: Vec<RosterRole>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamTemplateSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub category: Option<String>,
    /// 0 if roster JSON is invalid.
    pub role_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamTemplateDetail {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub category: Option<String>,
    pub role_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub roster: TeamRoster,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamsListResponse {
    pub teams: Vec<TeamTemplateSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamTemplateBody {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub roster: TeamRoster,
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectsListResponse {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectBody {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

// ---------------------------------------------------------------------------
// Chat (POST /api/v1/chat, OpenAI-shaped)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Assistant turn that asked for tools (OpenAI shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Set on `role: "tool"` result messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// A plain text turn — the shape every message had before tool calls.
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), ..Self::default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolFunction,
}

impl Default for ToolCall {
    fn default() -> Self {
        Self { id: String::new(), call_type: "function".into(), function: ToolFunction::default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolFunction {
    pub name: String,
    /// JSON-encoded arguments, streamed in fragments and concatenated.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionBody {
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider hint (`ollama`, `gemini`, …); the proxy routes to it and picks
    /// its default model when `model` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    /// OpenAI tool definitions, passed through by the server verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,
    /// Set by `sse::chat_stream`; leave `None` for the buffered `Client::chat`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

// ---------------------------------------------------------------------------
// System (mirrors web/src/api/system.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ReadinessCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadinessReport {
    pub ok: bool,
    pub status: String,
    pub checks: Vec<ReadinessCheck>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListeningOn {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessCounts {
    pub by_status: HashMap<String, i64>,
    pub active: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemPaths {
    pub database: Option<String>,
    pub database_backend: String,
    pub workspaces: Option<String>,
    pub llm_config_dir: Option<String>,
    pub model_ops_data: Option<String>,
}

/// `GET|PUT /system/resources` — how much of the machine the server may use, and
/// how much of that is in use right now (ADR 0010).
///
/// `mode` is what the user picked and `resolved` is what it currently means;
/// under `auto` the two differ, which is the whole reason both are on the wire.
#[derive(Debug, Clone, Deserialize)]
pub struct ResourcesView {
    pub mode: String,
    pub resolved: String,
    pub background_limit: usize,
    pub background_in_flight: usize,
    pub interactive_in_flight: usize,
    pub cpus: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemStatus {
    pub service: String,
    pub env: String,
    pub uptime_seconds: f64,
    /// `agent-platformd`'s own version. Was `python` (the interpreter version)
    /// until the Python child was retired and there was no interpreter left in
    /// the server to ask.
    pub server: String,
    pub platform: String,
    pub listening_on: ListeningOn,
    pub auth_required: bool,
    pub readiness: ReadinessReport,
    pub llm_proxy: ReadinessReport,
    pub processes: ProcessCounts,
    pub paths: SystemPaths,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogChunk {
    pub lines: Vec<String>,
    pub next: i64,
    pub dropped: i64,
}

// ---------------------------------------------------------------------------
// Model-ops (mirrors web/src/api/modelOps.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ModelRegistryEntry {
    pub id: i64,
    pub project_id: i64,
    #[serde(default)]
    pub project_name: Option<String>,
    pub version: String,
    pub ollama_tag: String,
    #[serde(default)]
    pub base_model: Option<String>,
    #[serde(default)]
    pub eval_score: Option<f64>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelProject {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub manifest: Value,
    pub registry_entries: Vec<ModelRegistryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelBuildJob {
    pub id: i64,
    pub job_type: String,
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub project_name: Option<String>,
    pub stages: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub current_stage: Option<String>,
    #[serde(default)]
    pub register_alias: Option<String>,
    pub result: Value,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub log_tail: Option<String>,
    pub poll_url: String,
    pub stream_url: String,
    pub created_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelProjectsResponse {
    pub projects: Vec<ModelProject>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelRegistryResponse {
    pub entries: Vec<ModelRegistryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadFilesResponse {
    pub uploaded: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModelsResponse {
    pub models: Vec<OllamaModelSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModelSummary {
    pub name: String,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub modified_at: Option<String>,
    #[serde(default)]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelProjectBody {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelBuildJobBody {
    pub project: String,
    pub stages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub register_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_eval: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// LLM providers (app/llm_proxy/admin_routes.py, master-key only)
// ---------------------------------------------------------------------------

/// One `.env` entry. Secrets come back as `set` + a `****abcd` tail and never
/// as a value, so the UI can show "configured" without holding the key.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnvKey {
    pub set: bool,
    #[serde(default)]
    pub masked: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderDefaults {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmEnv {
    pub keys: HashMap<String, EnvKey>,
    /// What is written in `.env` / config.yaml.
    pub persisted_defaults: ProviderDefaults,
    /// What the proxy will actually use, after falling back to a configured provider.
    pub resolved_defaults: ProviderDefaults,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderModels {
    pub options: Vec<String>,
    pub selected_model: String,
    /// `discovery` | `config_aliases` | `ui_fallback_models` | `provider_default` | `unavailable`.
    pub source: String,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub fallback_note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderEntry {
    pub id: String,
    pub label: String,
    pub configured: bool,
    pub local: bool,
    pub models: ProviderModels,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderCatalog {
    pub providers: Vec<ProviderEntry>,
    pub resolved_defaults: ProviderDefaults,
}

/// Only the keys the desktop offers. `AGENT_PLATFORM_MASTER_KEY` is deliberately
/// absent: the shell owns that key, and rewriting it would orphan this client.
/// Omitted fields are left untouched server-side.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnvUpdate {
    #[serde(rename = "GEMINI_API_KEY", skip_serializing_if = "Option::is_none")]
    pub gemini_api_key: Option<String>,
    #[serde(rename = "AIMLAPI_API_KEY", skip_serializing_if = "Option::is_none")]
    pub aimlapi_api_key: Option<String>,
    #[serde(rename = "AIMLAPI_OPENAI_BASE", skip_serializing_if = "Option::is_none")]
    pub aimlapi_openai_base: Option<String>,
    #[serde(rename = "OLLAMA_API_BASE", skip_serializing_if = "Option::is_none")]
    pub ollama_api_base: Option<String>,
    #[serde(rename = "LM_STUDIO_API_BASE", skip_serializing_if = "Option::is_none")]
    pub lm_studio_api_base: Option<String>,
    #[serde(rename = "LM_STUDIO_API_KEY", skip_serializing_if = "Option::is_none")]
    pub lm_studio_api_key: Option<String>,
    #[serde(rename = "ANTHROPIC_API_KEY", skip_serializing_if = "Option::is_none")]
    pub anthropic_api_key: Option<String>,
    /// Google Programmable Search credentials (ADR 0008's amendment) — not a
    /// chat provider, so no `ProviderMeta` row, but the desktop's Providers
    /// screen carries its own small card for these two, same write-only
    /// contract as the fields above.
    #[serde(rename = "SEARCH_API_KEY", skip_serializing_if = "Option::is_none")]
    pub search_api_key: Option<String>,
    /// Not a credential (`SENSITIVE_ENV_KEYS` on the server excludes it), so it
    /// round-trips as plain text through `EnvKey::value` rather than a mask.
    #[serde(rename = "SEARCH_CX", skip_serializing_if = "Option::is_none")]
    pub search_cx: Option<String>,
    #[serde(rename = "DEFAULT_PROVIDER", skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(rename = "DEFAULT_MODEL", skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvSaveResponse {
    pub ok: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Coder agent (mirrors app/coder/schemas.py)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct CoderThreadCreateOut {
    pub thread_id: i64,
    pub title: String,
    #[serde(default)]
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoderThreadSummary {
    pub id: i64,
    pub title: String,
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub preview: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoderThreadsListOut {
    pub threads: Vec<CoderThreadSummary>,
}

/// One thread with its whole history. `messages` stays `Value`: it is the raw
/// OpenAI-shaped log (user / assistant-with-`tool_calls` / tool), and the
/// desktop rebuilds its transcript from it rather than the server inventing a
/// second shape for the same thing.
#[derive(Debug, Clone, Deserialize)]
pub struct CoderThreadOut {
    pub thread_id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// The model the thread was answered on. The server pins it per thread and
    /// falls back to it whenever a turn does not name one.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub messages: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Workflows (mirrors app/workflows/schemas.py)
// ---------------------------------------------------------------------------

/// Steps stay `Value`: the server owns the schema, and the editor round-trips
/// raw JSON rather than re-validating a shape the API already validates.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowInfo {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<Value>,
    pub enabled: bool,
    pub interval_seconds: Option<i64>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowsListResponse {
    pub workflows: Vec<WorkflowInfo>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkflowBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<i64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clear_interval: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowStepResult {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub output: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowRunInfo {
    pub id: i64,
    pub workflow_id: i64,
    pub trigger: String,
    pub status: String,
    pub input: Value,
    pub steps: Vec<WorkflowStepResult>,
    pub error: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowRunsResponse {
    pub runs: Vec<WorkflowRunInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowAssistBody {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowAssistResponse {
    pub reply: String,
    /// Server-validated replacement steps; `None` means "no change proposed".
    pub steps: Option<Vec<Value>>,
}

// -- Todos -------------------------------------------------------------------

/// Item statuses, in board order. The server validates against the same list
/// (`TODO_STATUSES` in `app/todos/models.py`).
pub const TODO_STATUSES: [&str; 5] = ["plan", "backlog", "in_progress", "review", "done"];

#[derive(Debug, Clone, Deserialize)]
pub struct TodoBoardSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub category_count: i64,
    pub item_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TodoBoardsResponse {
    pub boards: Vec<TodoBoardSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TodoCategory {
    pub id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TodoItem {
    pub id: i64,
    pub category_id: Option<i64>,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: i64,
    pub tags: Vec<String>,
    pub due_at: Option<String>,
}

/// A board with everything on it — one request renders the whole kanban.
#[derive(Debug, Clone, Deserialize)]
pub struct TodoBoardDetail {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub categories: Vec<TodoCategory>,
    pub items: Vec<TodoItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodoBoardBody {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodoItemBody {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<i64>,
}

/// Partial update: only the fields present are changed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TodoItemPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<i64>,
}

// -- Personal assistant ------------------------------------------------------

/// Horizons `GET /assistant/dashboard` groups by (`_horizon_range` server-side).
pub const ASSISTANT_HORIZONS: [&str; 3] = ["day", "week", "month"];

/// The assistant's own board for one project, sliced by horizon. Items are the
/// same `ItemOut` the todo API returns, so [`TodoItem`] deserializes them.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantDashboard {
    pub project_id: i64,
    pub board_id: i64,
    pub horizon: String,
    pub categories: Vec<TodoCategory>,
    pub items: Vec<TodoItem>,
    pub overdue: Vec<TodoItem>,
    pub habits_due: Vec<TodoItem>,
    pub goals: Vec<TodoItem>,
    pub stats: AssistantStats,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssistantStats {
    pub total_items: i64,
    pub done_count: i64,
    pub active_count: i64,
    pub overdue_count: i64,
    pub habits_due_count: i64,
}

/// One action the assistant proposes. `parameters` is carried opaquely because
/// applying an action means handing the server back the *same* object it sent —
/// seventeen action ids with seventeen parameter shapes, none of which this
/// client has any reason to understand.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlannedAction {
    pub action_id: String,
    pub name: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default = "one")]
    pub confidence: f64,
    #[serde(default)]
    pub reasoning: Option<String>,
}

fn one() -> f64 {
    1.0
}

/// A pending review. `POST /reviews/run` returns the same fields under
/// `review_id` rather than `id`, so both names deserialize.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantReview {
    #[serde(alias = "review_id")]
    pub id: i64,
    pub status: String,
    pub summary: Option<String>,
    #[serde(default)]
    pub proposed_actions: Vec<PlannedAction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantReviewsResponse {
    pub reviews: Vec<AssistantReview>,
}

// -- Personal assistant: the planning chat -----------------------------------

/// A thread in the sidebar picker. `preview` is the last thing *the user* said,
/// which is what makes one thread tellable from another.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantThreadSummary {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantThreadsResponse {
    #[serde(default)]
    pub threads: Vec<AssistantThreadSummary>,
}

/// `POST /assistant/chat/threads` — the only route that answers with a bare id
/// rather than a thread body.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantThreadCreated {
    pub thread_id: i64,
}

/// One stored turn. `proposed_actions` is the snapshot of what the assistant
/// offered *at that point in the thread*, and `proposal_status` says what became
/// of it (`pending`, `approved`, `dismissed`, `superseded`) — so a reopened
/// thread shows a decision that was already taken as taken.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub proposed_actions: Vec<PlannedAction>,
    #[serde(default)]
    pub proposal_status: Option<String>,
}

/// One control in a planning form. `kind` is one of `boolean`, `single_select`,
/// `multi_select`, `text`, `textarea` — anything else renders as text, because a
/// field the client cannot draw must still be answerable.
#[derive(Debug, Clone, Deserialize)]
pub struct PlanningFormField {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default, rename = "helpText")]
    pub help_text: Option<String>,
    /// Prefilled from the stored profile, so a re-asked field is not re-typed.
    #[serde(default)]
    pub default: Option<Value>,
}

/// The intake or clarifying form an action asked for. `purpose == "clarifying"`
/// means the answers go back as a chat turn; anything else saves to the domain
/// profile first.
#[derive(Debug, Clone, Deserialize)]
pub struct PlanningForm {
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub fields: Vec<PlanningFormField>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContextUsage {
    #[serde(default)]
    pub context_window: i64,
    #[serde(default)]
    pub total_estimated: i64,
    #[serde(default)]
    pub percent_used: f64,
}

/// One type for five routes. `GET /chat/thread`, `POST /chat/send`, `/retry` and
/// `/submit-form` all answer with a thread and differ only in which extras they
/// carry, so every field that is not universal defaults.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssistantChatThread {
    #[serde(default)]
    pub thread_id: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub messages: Vec<AssistantChatMessage>,
    #[serde(default)]
    pub pending_actions: Vec<PlannedAction>,
    #[serde(default)]
    pub pending_form: Option<PlanningForm>,
    #[serde(default)]
    pub context_usage: Option<ContextUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantChatSend {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantChatRetry {
    pub thread_id: i64,
    pub message_index: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantFormSubmit {
    pub domain: String,
    pub answers: HashMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    pub auto_continue: bool,
}

/// Applying an empty action list is how a proposal is *dismissed* — the server
/// resolves the thread's pending snapshot either way.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantApplyBody {
    pub actions: Vec<PlannedAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    pub auto_continue: bool,
}

/// What the board did. Read even though the thread is refetched right after:
/// the auto-continue turn that would have narrated this is allowed to fail, and
/// then this is the only record the user gets.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssistantApplyResult {
    #[serde(default)]
    pub applied: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<String>,
    #[serde(default)]
    pub guidance: Vec<String>,
    /// The turn the assistant took *about* the apply. Present means the summary
    /// is already in the transcript and does not need repeating; the body itself
    /// is not read, because the thread is refetched anyway.
    #[serde(default)]
    pub continuation: Option<Value>,
}

// -- Web search (ADR 0008, docs/web-search-module-plan.md) -------------------

/// Mirrors the server's `DorkQuery` (`server/src/search_dork.rs`) field for
/// field. Deserialization target for a response's `parts` only — rendering
/// the operator string is the server's job alone
/// (`GET /api/v1/search/dork?q=…&drop=…`), so removing a chip re-runs the
/// search against the server rather than re-deriving the grammar here. See
/// `docs/web-search-module-plan.md`. **Must not regain any rendering or
/// operator-spelling logic** — that was deliberately deleted from this crate;
/// no dork grammar exists outside the server's `search_dork.rs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DorkParts {
    #[serde(default)]
    pub terms: String,
    #[serde(default)]
    pub exact: Vec<String>,
    #[serde(default)]
    pub any_of: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub exclude_sites: Vec<String>,
    /// `related:` — sites similar to this one.
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub filetype: Option<String>,
    #[serde(default)]
    pub intitle: Vec<String>,
    /// `intext:` — the page body must contain this.
    #[serde(default)]
    pub intext: Vec<String>,
    #[serde(default)]
    pub inurl: Vec<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
    /// Google's `lo..hi` numeric range operator, kept as strings same as
    /// `after`/`before` — this struct never evaluates the value, only carries it.
    #[serde(default)]
    pub range: Option<(String, String)>,
}

/// One removable chip — mirrors the server's `DorkChip`
/// (`server/src/search_dork.rs::DorkQuery::chips`). `token` is handed back
/// verbatim as `drop=` on the next request; `label` is display text with the
/// dork operator prefix already stripped; `field` names the `DorkParts`
/// field it came from (`sites`, `exclude`, `exact`, `any_of`, `intitle`,
/// `inurl`, `exclude_sites`, `filetype`, `after`, `before`) so the client can
/// pick a tone without parsing dork syntax.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DorkChip {
    pub token: String,
    pub label: String,
    pub field: String,
}

/// One line of `search.rs`'s `explanation` array. `kind` is `"recipe"` (a
/// fired intent recipe's own sentence — leads, rendered stronger) or
/// `"operator"` (one per operator [`DorkParts::render`] can emit — follows,
/// rendered lighter); see `server/src/search.rs`'s doc comment on why the two
/// must not be folded into one shape.
#[derive(Debug, Clone, Deserialize)]
pub struct DorkExplanationLine {
    pub kind: String,
    pub label: String,
    pub meaning: String,
}

/// `GET /api/v1/search/dork`'s full response.
#[derive(Debug, Clone, Deserialize)]
pub struct DorkResponse {
    pub query: String,
    pub url: String,
    pub engine: SearchEngine,
    /// `"verbatim"` (from `q=`) | `"rules"` (a recipe fired) | `"model"` (the
    /// server's own LLM translated it) — display only.
    pub source: String,
    #[serde(default)]
    pub recipes: Vec<String>,
    pub parts: DorkParts,
    #[serde(default)]
    pub explanation: Vec<DorkExplanationLine>,
    /// One removable chip per element in `parts` — see [`DorkChip`]. The
    /// client renders these directly rather than re-deriving dork grammar
    /// from `parts` itself.
    #[serde(default)]
    pub chips: Vec<DorkChip>,
}

/// One result row from `GET /api/v1/search` (ADR 0008's amendment) — mirrors
/// `server/src/search.rs::parse_cse_body`'s wire shape exactly.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub domain: String,
    pub snippet: String,
}

/// `GET /api/v1/search`'s full response — everything [`DorkResponse`] answers
/// (`#[serde(flatten)]`, since the server builds both from the same
/// `dork_body` map) plus the amendment's three fields. `configured: false` is
/// the default install and is **not** an error and **not** "no matches" —
/// see `server/src/search.rs`'s module doc comment and the ADR amendment
/// "results, behind a key". A caller must check `configured` before trusting
/// `results`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    #[serde(flatten)]
    pub dork: DorkResponse,
    pub configured: bool,
    #[serde(default)]
    pub results: Vec<SearchResult>,
    #[serde(default)]
    pub total_estimate: Option<i64>,
}

// -- Web search: history (`/api/v1/search/history`) --------------------------

/// One row of `search_history` — mirrors `server/src/search.rs`'s
/// `SearchHistoryOut`. `opened` is a wire boolean (the server's `sql_flag`
/// serializer turns its `INTEGER` column into one); see that module's doc
/// comment for why the column itself cannot be a `bool`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHistoryEntry {
    pub id: i64,
    pub workspace_id: Option<i64>,
    pub query: String,
    pub engine: String,
    pub source: String,
    pub opened: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHistoryResponse {
    pub history: Vec<SearchHistoryEntry>,
}

// -- Media generation (`/api/v1/media/*`, ADR 0009) --------------------------

/// Whether the local ComfyUI backend is answering, and what it can draw with.
/// `reachable: false` is the ordinary state on an install that has not set it
/// up — the Studio screen renders an install pointer, never an error.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaStatus {
    pub reachable: bool,
    pub base: String,
    #[serde(default)]
    pub checkpoints: Vec<String>,
    /// The checkpoint the server would use for the next image, chosen from
    /// `checkpoints` — `None` when none is installed.
    pub image_model: Option<String>,
}

/// One row of `media_jobs` — mirrors `server/src/media.rs`'s `MediaJobOut`.
/// `status` is queued | running | completed | failed; `file_name` is set only
/// once the output has been copied into the server's media folder, and the
/// bytes come from `GET /api/v1/media/jobs/{id}/file`.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaJob {
    pub id: i64,
    pub kind: String,
    pub prompt: String,
    pub enhanced_prompt: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub width: i64,
    pub height: i64,
    pub length: i64,
    pub seed: i64,
    pub file_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl MediaJob {
    pub fn is_video(&self) -> bool {
        self.kind == "video"
    }

    /// Still working — the desktop polls while any job says so.
    pub fn is_running(&self) -> bool {
        self.status == "queued" || self.status == "running"
    }

    pub fn is_done(&self) -> bool {
        self.status == "completed" && self.file_name.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaJobsResponse {
    pub jobs: Vec<MediaJob>,
}

/// One file `text_to_video.json` names by hand, and whether ComfyUI has it.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaRequirement {
    /// The ComfyUI models subfolder it belongs in (`vae`, `diffusion_models`,
    /// `text_encoders`) — also the directory a download writes to.
    pub folder: String,
    pub file_name: String,
    pub url: String,
    pub size_bytes: i64,
    pub installed: bool,
}

/// The body of `GET /api/v1/media/requirements`.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaRequirements {
    /// ComfyUI's `models/` directory, or `None` when the server could not
    /// establish it — nothing may be written anywhere in that case.
    pub models_root: Option<String>,
    pub items: Vec<MediaRequirement>,
}

impl MediaRequirements {
    /// Everything still to fetch, in the order the server listed it.
    pub fn missing(&self) -> impl Iterator<Item = &MediaRequirement> {
        self.items.iter().filter(|i| !i.installed)
    }

    /// Bytes the user would be spending, for the confirm step.
    pub fn missing_bytes(&self) -> i64 {
        self.missing().map(|i| i.size_bytes).sum()
    }

    /// Whether a download may even be offered: something to fetch, and a
    /// verified directory to put it in.
    pub fn can_install(&self) -> bool {
        self.models_root.is_some() && self.missing().next().is_some()
    }
}

/// The body of `GET /api/v1/media/suggest` — one ready-to-run prompt.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaSuggestion {
    pub kind: String,
    pub prompt: String,
}

/// The body of `POST /api/v1/media/generate` — mirrors `media.rs`'s
/// `GenerateRequest`. Omitted fields take the server's per-kind defaults
/// (1024² for an image, 832×480 and 49 frames for a video).
#[derive(Debug, Clone, Serialize)]
pub struct MediaGenerateRequest {
    pub kind: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<i64>,
    pub enhance: bool,
}

#[cfg(test)]
mod dork_tests {
    use super::*;

    #[test]
    fn a_dork_response_decodes() {
        let body = r#"{"query":"filetype:pdf intitle:\"Attention Is All You Need\"",
            "url":"https://www.google.com/search?q=x","engine":"google","source":"rules",
            "recipes":["document"],
            "parts":{"terms":"","filetype":"pdf","intitle":["Attention Is All You Need"]},
            "explanation":[{"kind":"recipe","label":"document","meaning":"Looking for a document"},
                           {"kind":"operator","label":"filetype:pdf","meaning":"only results of file type pdf"}],
            "chips":[{"token":"filetype:pdf","label":"pdf","field":"filetype"},
                     {"token":"intitle:\"Attention Is All You Need\"","label":"Attention Is All You Need","field":"intitle"}]}"#;
        let r: DorkResponse = serde_json::from_str(body).unwrap();
        assert_eq!(r.source, "rules");
        assert_eq!(r.engine, SearchEngine::Google);
        assert_eq!(r.parts.filetype.as_deref(), Some("pdf"));
        assert_eq!(r.explanation[0].kind, "recipe");
        assert_eq!(r.explanation[1].kind, "operator");
        assert_eq!(r.chips.len(), 2);
        assert_eq!(r.chips[0].token, "filetype:pdf");
        assert_eq!(r.chips[0].field, "filetype");
    }

    /// The unconfigured path (ADR 0008's amendment) — everything `/dork`
    /// answers is still present, flattened alongside `configured: false` and
    /// an empty `results`.
    #[test]
    fn a_search_response_decodes_when_unconfigured() {
        let body = r#"{"query":"cheap mechanical keyboard",
            "url":"https://www.google.com/search?q=x","engine":"google","source":"verbatim",
            "recipes":[],"parts":{"terms":"cheap mechanical keyboard"},"explanation":[],"chips":[],
            "configured":false,"results":[],"total_estimate":null}"#;
        let r: SearchResponse = serde_json::from_str(body).unwrap();
        assert!(!r.configured);
        assert!(r.results.is_empty());
        assert_eq!(r.total_estimate, None);
        // The flattened dork fields are still the real translation, not a stub.
        assert_eq!(r.dork.query, "cheap mechanical keyboard");
        assert_eq!(r.dork.source, "verbatim");
    }

    /// The keyed path: `results` and `total_estimate` populated alongside the
    /// same dork fields `/dork` would have answered.
    #[test]
    fn a_search_response_decodes_with_results() {
        let body = r#"{"query":"keyboard","url":"https://www.google.com/search?q=x",
            "engine":"google","source":"verbatim","recipes":[],"parts":{"terms":"keyboard"},
            "explanation":[],"chips":[],"configured":true,
            "results":[{"title":"t","url":"https://example.com","domain":"example.com","snippet":"s"}],
            "total_estimate":12400}"#;
        let r: SearchResponse = serde_json::from_str(body).unwrap();
        assert!(r.configured);
        assert_eq!(r.results.len(), 1);
        assert_eq!(r.results[0].domain, "example.com");
        assert_eq!(r.total_estimate, Some(12400));
    }

    #[test]
    fn a_search_history_list_decodes() {
        let body = r#"{"history":[{"id":1,"workspace_id":null,"query":"keyboard",
            "engine":"google","source":"verbatim","opened":true,"created_at":"2026-08-15T00:00:00"}]}"#;
        let r: SearchHistoryResponse = serde_json::from_str(body).unwrap();
        assert_eq!(r.history.len(), 1);
        assert!(r.history[0].opened);
    }
}
