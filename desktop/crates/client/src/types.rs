//! Contracts for agent-platform FastAPI JSON responses.
//! Mirrors `web/src/api/types.ts` / `web/src/api/system.ts` / `web/src/api/modelOps.ts`,
//! which in turn track `app/models.py` and route payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub use crate::enums_gen::*;

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

#[derive(Debug, Clone, Serialize, Default)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Assistant turn that asked for tools (OpenAI shape).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Set on `role: "tool"` result messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// A plain text turn — the shape every message had before tool calls.
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), ..Self::default() }
    }
}

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize, Default)]
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

#[derive(Debug, Clone, Deserialize)]
pub struct SystemStatus {
    pub service: String,
    pub env: String,
    pub uptime_seconds: f64,
    pub python: String,
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
