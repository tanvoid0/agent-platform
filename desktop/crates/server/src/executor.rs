//! The DAG executor: planner call, wave loop, task execution, sub-DAG
//! expansion, and startup recovery. Port of `app/orchestrator.py`, the twelve
//! `app/services/*` modules this domain owns, `app/process_approval.py`,
//! `app/services/startup_recovery.py` and `app/context_summarize.py`.
//!
//! # What the routes call
//!
//! Four fire-and-forget entry points, because that is exactly what FastAPI's
//! `BackgroundTasks` is here — the handler writes a status, returns, and the
//! work happens after the response:
//! [`spawn_plan`], [`spawn_execute_dag`], [`spawn_expand_after_review`],
//! [`spawn_startup_recovery`]. Each wraps its future in `catch_unwind`, so a bug
//! in here fails one process instead of taking the daemon down with it.
//!
//! # The wave loop
//!
//! `execute_dag` is **not** a task graph. Per wave it re-reads the process, runs
//! `sync_review_assignments`, checks cancelled / failed / the run budget,
//! computes ready ids **FIFO by `tasknode.id`** capped by
//! `AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS`, and runs that wave in a `JoinSet`.
//!
//! **Cancellation and pause are DB-mediated, not in-process.** `POST /cancel`
//! and `POST /sync` write a status and this loop notices it at the top of the
//! next wave. There is deliberately no channel, no `CancellationToken` and no
//! shared flag: a loop that only reads the database is the thing that makes a
//! half-migrated server survivable, because Python's routes can still steer it.
//!
//! # The tool-calling path is dead by default
//!
//! `AGENT_PLATFORM_TOOLS_ENABLED` is unset and `ToolPolicy::is_allowed` returns
//! false on an empty allowlist, so `_invoke_task_llm`'s tool branch never ran in
//! this deployment. [`load_policy`] is ported (three env reads) and the task
//! **refuses to run** when the branch would fire, rather than quietly answering
//! without the tools the operator asked for. That kept `tool_handlers.py`'s 782
//! LOC out of the port — and when the Python server was deleted, that file and
//! the MCP client only reachable through it went with it. The refusal is
//! permanent now rather than a placeholder.
//!
//! # Deliberate divergences
//!
//! - **`record_api_token_usage` writes `SET x = x + 1`** where Python does a
//!   read-modify-write. Rust's is the safe one; Python's is not, and
//!   `assistant/`, `coder/` and `playground/` still called it from that side until
//!   step 4 — so the lost-update window this step opens is theirs, not ours.
//! - **Error text.** Python's `LLM*Error` messages come from `llm_client.py`'s
//!   HTTP layer, which is not ported; `llm::complete_internal` carries its own.
//!   `process.failure_reason` therefore reads differently on a failed run.
//! - **`failure_debug_json` carries no Python traceback** and its
//!   `exception_type` names the Rust failure kind.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use tokio::task::JoinSet;

use crate::context_budget::{
    dependency_context_token_budget, estimate_tokens, fit_chat_messages_for_request,
    fit_dependency_outputs_to_budget, max_output_tokens_default, subdag_parent_output_max_tokens,
    truncate_text_to_tokens,
};
use crate::dag_schema::{
    merge_planner_with_new_subagents, planner_dag_to_json, py_repr, python_json,
    sanitize_llm_model_alias, validate_planner_dag, validate_subagent, PlannerDag, SubagentSpec,
};
use crate::teams::TeamRoster;
use crate::wire::sql_now;
use crate::{env_opt, AppState};

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// `raw in ("1", "true", "yes", "on")` after `.strip().lower()`.
fn env_flag(name: &str, default: bool) -> bool {
    match env_opt(name) {
        None => default,
        Some(raw) => matches!(raw.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
    }
}

/// `float(raw)` when it parses and is positive, else `None` — the shape
/// `_plan_timeout_seconds` and `_run_max_seconds` share.
fn env_positive_f64(name: &str) -> Option<f64> {
    env_opt(name)?.parse::<f64>().ok().filter(|v| *v > 0.0)
}

/// `int(raw)` when it parses and is positive, else `None` (unlimited).
fn env_positive_i64(name: &str) -> Option<i64> {
    env_opt(name)?.parse::<i64>().ok().filter(|v| *v > 0)
}

/// `max(0, int(raw))`, falling back to `default` when unset and to **0** when
/// set to something unparsable — Python's `except ValueError: return 0`, which
/// disables expansion rather than using the default.
fn env_count_or_zero(name: &str, default: i64) -> i64 {
    match env_opt(name) {
        None => default,
        Some(raw) => raw.parse::<i64>().map(|v| v.max(0)).unwrap_or(0),
    }
}

fn plan_timeout_seconds() -> Option<f64> {
    env_positive_f64("AGENT_PLATFORM_PLAN_TIMEOUT_SECONDS")
}

fn run_max_seconds() -> Option<f64> {
    env_positive_f64("AGENT_PLATFORM_RUN_MAX_SECONDS")
}

/// Cap on dependency-ready tasks per wave. Unset or non-positive is unlimited.
fn max_concurrent_tasks() -> Option<usize> {
    env_positive_i64("AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS").map(|v| v as usize)
}

/// Default > 0 so planner `subdecompose` nodes can spawn follow-on work without
/// extra env.
fn subdecomp_max_expansions() -> i64 {
    env_count_or_zero("AGENT_PLATFORM_SUBDECOMP_MAX_EXPANSIONS", 48)
}

fn subdecomp_max_new_tasks() -> i64 {
    env_count_or_zero("AGENT_PLATFORM_SUBDECOMP_MAX_NEW_TASKS", 48)
}

/// Tasks added by an expansion have depth parent+1; planner tasks are depth 0.
fn subdecomp_max_depth() -> Option<i64> {
    env_positive_i64("AGENT_PLATFORM_SUBDECOMP_MAX_DEPTH")
}

fn env_auto_approve() -> bool {
    env_flag("AGENT_PLATFORM_AUTO_APPROVE", false)
}

fn resume_on_startup_enabled() -> bool {
    env_flag("AGENT_PLATFORM_RESUME_ON_STARTUP", true)
}

/// Total LLM calls allowed for planning when JSON/schema validation fails.
fn plan_max_attempts() -> usize {
    match env_opt("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS") {
        None => 3,
        Some(raw) => raw.parse::<i64>().map(|v| v.max(1) as usize).unwrap_or(3),
    }
}

fn default_planner_model() -> Option<String> {
    sanitize_llm_model_alias(&env_opt("PLANNER_MODEL")?)
}

pub(crate) fn default_subagent_model() -> Option<String> {
    match env_opt("SUBAGENT_MODEL") {
        Some(raw) => sanitize_llm_model_alias(&raw),
        None => default_planner_model(),
    }
}

/// A stronger alias used only on the last attempt, when it is set and differs
/// from `PLANNER_MODEL`.
fn planner_fallback_model() -> Option<String> {
    let fallback = sanitize_llm_model_alias(&env_opt("PLANNER_FALLBACK_MODEL")?)?;
    match default_planner_model() {
        Some(primary) if primary == fallback => None,
        _ => Some(fallback),
    }
}

fn model_for_plan_attempt(
    attempt: usize,
    max_attempts: usize,
    fallback: Option<&str>,
) -> Option<String> {
    match fallback {
        Some(fallback) if attempt + 1 == max_attempts => Some(fallback.to_string()),
        _ => default_planner_model(),
    }
}

// ---------------------------------------------------------------------------
// Tool policy (`app/tools_policy.py`)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct ToolPolicy {
    enabled: bool,
    allowlist: Vec<String>,
    budget_per_run: i64,
}

/// `tools_enabled()` compares the **raw** value against `("1", "true", "yes")`
/// with no lowercasing, so `TRUE` does not enable tools. Ported as written.
fn load_policy() -> ToolPolicy {
    ToolPolicy {
        enabled: matches!(
            env_opt("AGENT_PLATFORM_TOOLS_ENABLED").as_deref(),
            Some("1" | "true" | "yes")
        ),
        allowlist: env_opt("AGENT_PLATFORM_TOOLS_ALLOWLIST")
            .map(|raw| {
                raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
            })
            .unwrap_or_default(),
        budget_per_run: match env_opt("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN") {
            None => 0,
            Some(raw) => raw.parse::<i64>().map(|v| v.max(0)).unwrap_or(0),
        },
    }
}

/// `_invoke_task_llm`'s `remaining_tool_budget`: how many tool invocations this
/// task may make. Zero takes the plain-completion branch, which is the only one
/// this port implements.
fn remaining_tool_budget(policy: &ToolPolicy, used: i64) -> i64 {
    if policy.enabled && !policy.allowlist.is_empty() && policy.budget_per_run > 0 {
        (policy.budget_per_run - used).max(0)
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// LLM plumbing
// ---------------------------------------------------------------------------

/// Which of Python's two `except` arms this failure would have landed in.
#[derive(Debug)]
enum LlmFailure {
    /// The `LLMConfigurationError | LLMAuthenticationError | LLMTransportError |
    /// LLMRequestError` arm: `failure_debug_json.source == "llm"`.
    Llm(String),
    /// The bare `except Exception` arm: `source == "unexpected"`.
    Unexpected(String),
}

impl std::fmt::Display for LlmFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmFailure::Llm(m) | LlmFailure::Unexpected(m) => f.write_str(m),
        }
    }
}

impl LlmFailure {
    fn source(&self) -> &'static str {
        match self {
            LlmFailure::Llm(_) => "llm",
            LlmFailure::Unexpected(_) => "unexpected",
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            LlmFailure::Llm(_) => "LLMRequestError",
            LlmFailure::Unexpected(_) => "RuntimeError",
        }
    }
}

struct Completion {
    content: String,
    tokens: i64,
    cost: f64,
}

/// USD cost from an OpenAI-compatible chat completion body. Port of
/// `llm_client.usage_cost_from_completion_response`: LiteLLM, OpenRouter and
/// friends each put it somewhere different, and a backend that reports none
/// (plain Ollama) is 0.0, not an error.
pub(crate) fn usage_cost_from_completion_response(data: &Value) -> f64 {
    fn coerce(v: Option<&Value>) -> Option<f64> {
        match v? {
            Value::Bool(_) => None,
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }
    fn nested(v: Option<&Value>) -> Option<f64> {
        match v {
            Some(Value::Object(map)) => ["total_cost", "cost"]
                .iter()
                .filter(|k| map.contains_key(**k))
                .find_map(|k| coerce(map.get(*k))),
            other => coerce(other),
        }
    }

    let usage = data.get("usage").and_then(Value::as_object);
    if let Some(usage) = usage {
        for key in ["cost", "total_cost"] {
            if usage.contains_key(key) {
                if let Some(c) = coerce(usage.get(key)) {
                    return c;
                }
            }
        }
        if let Some(c) = nested(usage.get("response_cost")) {
            return c;
        }
    }
    if let Some(c) = nested(data.get("response_cost")) {
        return c;
    }
    if let Some(hidden) = data.get("_hidden_params").and_then(Value::as_object) {
        if let Some(c) = coerce(hidden.get("response_cost")) {
            return c;
        }
    }
    0.0
}

/// `llm_client.call_llm`, minus the loopback HTTP hop: the same request body,
/// handed to [`crate::llm::complete_internal`] instead of posted to
/// `/v1/chat/completions`. `response_format`, `temperature` and `max_tokens` pass
/// straight through, because that function takes the request object itself.
async fn call_llm(
    state: &AppState,
    messages: &[(&str, String)],
    model: Option<&str>,
    require_json: bool,
    temperature: f64,
    max_output_tokens: Option<i64>,
) -> Result<Completion, LlmFailure> {
    let raw_model = model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .or_else(default_subagent_model);
    let resolved = raw_model.as_deref().and_then(sanitize_llm_model_alias);

    let (fitted, _) = fit_chat_messages_for_request(
        messages.iter().map(|(role, content)| json!({"role": role, "content": content})).collect(),
    );

    let mut body = Map::new();
    body.insert("messages".into(), Value::Array(fitted));
    body.insert("temperature".into(), json!(temperature));
    if let Some(model) = resolved {
        body.insert("model".into(), json!(model));
    }
    body.insert(
        "max_tokens".into(),
        json!(max_output_tokens.unwrap_or_else(max_output_tokens_default)),
    );
    if require_json {
        body.insert("response_format".into(), json!({"type": "json_object"}));
    }

    let data = crate::llm::complete_internal(state, body)
        .await
        .map_err(|e| LlmFailure::Llm(e.message))?;

    // Python indexes `data["choices"][0]["message"]["content"]`, so a body
    // without that path is a `KeyError` and lands in the "unexpected" arm.
    let message = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .ok_or_else(|| {
            LlmFailure::Unexpected("LLM response had no choices[0].message".to_string())
        })?;
    if !message.as_object().is_some_and(|m| m.contains_key("content")) {
        return Err(LlmFailure::Unexpected(
            "LLM response had no choices[0].message.content".to_string(),
        ));
    }
    // A non-string `content` (a `null` from a tool-only turn) is `None` in
    // Python and then written straight into a NOT NULL column; empty is the
    // nearest thing that does not corrupt the row.
    let content = message.get("content").and_then(Value::as_str).unwrap_or("").to_string();

    let tokens = data
        .get("usage")
        .and_then(|u| u.get("total_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);

    Ok(Completion { content, tokens, cost: usage_cost_from_completion_response(&data) })
}

// ---------------------------------------------------------------------------
// Context summarisation (`app/context_summarize.py`)
// ---------------------------------------------------------------------------

const SUMMARY_PROMPT: &str = "You compress prior-step outputs for a downstream agent. Preserve:
- concrete facts, numbers, names, paths, URLs, errors
- decisions and conclusions
- anything needed to continue the task

Be concise. Use bullet lists when helpful. Omit filler and repetition.";

fn summarize_int(name: &str, default: i64, min: i64) -> usize {
    match env_opt(name) {
        None => default as usize,
        Some(raw) => raw.parse::<i64>().map(|v| v.max(min)).unwrap_or(default) as usize,
    }
}

/// Replace an oversized dependency chunk with an LLM summary, when
/// `AGENT_PLATFORM_CONTEXT_SUMMARIZE` is on. Otherwise the text is returned
/// unchanged and the caller's token budget does the trimming.
async fn maybe_condense_text_for_context(
    state: &AppState,
    text: &str,
    model: Option<&str>,
) -> Result<String, LlmFailure> {
    if !env_flag("AGENT_PLATFORM_CONTEXT_SUMMARIZE", false) {
        return Ok(text.to_string());
    }
    let min_input = summarize_int("AGENT_PLATFORM_CONTEXT_SUMMARIZE_MIN_TOKENS", 6000, 512);
    if estimate_tokens(text) < min_input {
        return Ok(text.to_string());
    }
    let max_input = summarize_int("AGENT_PLATFORM_CONTEXT_SUMMARIZE_MAX_INPUT_TOKENS", 16000, 1024);
    let max_output =
        summarize_int("AGENT_PLATFORM_CONTEXT_SUMMARIZE_MAX_OUTPUT_TOKENS", 900, 128);

    let body = truncate_text_to_tokens(text, max_input);
    let messages =
        [("system", SUMMARY_PROMPT.to_string()), ("user", body)];
    let out = call_llm(state, &messages, model, false, 0.2, Some(max_output as i64)).await?;
    let trimmed = out.content.trim();
    if trimmed.is_empty() {
        Ok(truncate_text_to_tokens(text, max_output))
    } else {
        Ok(trimmed.to_string())
    }
}

// ---------------------------------------------------------------------------
// Planner prompts
// ---------------------------------------------------------------------------

/// Verbatim from `llm_client.generate_planner_dag`. Every character of this is
/// what the model has been tuned against; it is not documentation.
const PLANNER_SYSTEM_PROMPT: &str = r#"You are an elite Agentic Team Planner. Decompose the user's goal into a DAG of subagents.

When the user message includes a "Preferred team roster" section, **prefer** aligning subagent `role` names and
splitting work along those roles when it fits the goal; you may still add or merge roles if the goal requires it.

**Granularity:** Prefer **more, smaller** subagents over few monolithic ones. Each subagent should own one
outcome (research slice, integration step, doc section, test pass, refactor chunk). Put **independent**
work in **separate** nodes with **empty or minimal** dependencies so they can run in parallel in the
same wave when possible. Use dependencies only for true ordering (B needs A's output).

**model field:** Omit `model` unless you use a real model alias from the server. Never put role titles,
programming languages, or skill labels (e.g. `typescript-expert`, `react-scaffolder`) in `model`—those belong in `role` / prompts only.

**subdecompose:** Set `subdecompose`: true on nodes whose deliverable is likely to reveal follow-on work
after execution (e.g. exploration, broad research, scaffolding) so the system can add subtasks from the
completed output. Omit or false for tight, predictable leaf tasks.

Output valid JSON strictly adhering to this schema:
{
  "team_name": "Name of the team",
  "goal_restatement": "What we are doing",
  "subagents": [
    {
      "client_uuid": "A unique string id like 'agent_1'",
      "role": "e.g. Researcher, Synthesizer",
      "system_prompt": "Identity and boundaries of the agent",
      "instructions": "Specific task for this agent. Mention that it will receive context from dependencies.",
      "dependencies": ["client_uuid_of_prior_agent"],
      "model": "optional; real model alias only (e.g. gemma4, gemini-flash). Omit to use server default. Never use role or skill slugs (e.g. typescript-expert, react-scaffolder).",
      "subdecompose": "optional boolean; if true, after this node completes the server may append child subtasks from its output (within AGENT_PLATFORM_SUBDECOMP_* limits).",
      "requires_review": "optional boolean; if true, execution pauses for human review after this node's output."
    }
  ]
}
Make sure all dependencies are valid client_uuids from the subagents list. Ensure no circular dependencies.
"#;

/// Verbatim from `llm_client.generate_subdag_expansion`, with `{parent_uuid}`
/// interpolated exactly where the f-string put it.
fn subdag_system_prompt(parent_uuid: &str) -> String {
    format!(
        r#"You extend an existing execution DAG. Output JSON only with this shape:
{{
  "subagents": [
    {{
      "client_uuid": "new unique id (never reuse existing UUIDs)",
      "role": "short role name",
      "system_prompt": "identity and boundaries",
      "instructions": "single, concrete deliverable for this subagent",
      "dependencies": ["must include "{parent_uuid}" and may include other new UUIDs"],
      "model": "optional real model alias only; omit unless you know the proxy name (never role slugs like typescript-expert or react-scaffolder)",
      "subdecompose": "optional boolean; true if this subagent's output may justify further split tasks later",
      "requires_review": "optional boolean; true only if human gate needed before dependents run"
    }}
  ]
}}
Rules:
- Prefer **many small parallel subagents** over one large step: each node should complete one clear artifact
  (file slice, API section, research angle, test batch, doc section). Peers that do not depend on each
  other should **not** list each other—only list "{parent_uuid}" or prior new UUIDs they truly need.
- At least one new subagent (more is better when the parent output has separable work).
- Every new subagent MUST list "{parent_uuid}" in dependencies (direct dependency on the parent task).
- client_uuid values must be unique and MUST NOT be any of the existing UUIDs.
- Dependencies may only reference "{parent_uuid}" or client_uuids from your new subagents (acyclic).
"#
    )
}

fn planner_user_message(goal: &str, team_context: Option<&str>) -> String {
    let mut parts = vec![format!("Goal: {goal}")];
    if let Some(context) = team_context.map(str::trim).filter(|c| !c.is_empty()) {
        parts.push(String::new());
        parts.push(context.to_string());
    }
    parts.join("\n")
}

struct PlanOutcome {
    dag: PlannerDag,
    tokens: i64,
    cost: f64,
}

/// The JSON-repair retry loop: up to `AGENT_PLATFORM_PLAN_MAX_ATTEMPTS` calls,
/// with `PLANNER_FALLBACK_MODEL` on the last one. Tokens and cost accumulate
/// across every attempt, including the discarded ones — a failed parse still
/// cost money.
async fn generate_planner_dag(
    state: &AppState,
    goal: &str,
    team_context: Option<&str>,
) -> Result<PlanOutcome, LlmFailure> {
    let messages = [
        ("system", PLANNER_SYSTEM_PROMPT.to_string()),
        ("user", planner_user_message(goal, team_context)),
    ];

    let max_attempts = plan_max_attempts();
    let fallback = planner_fallback_model();
    let mut tokens = 0i64;
    let mut cost = 0.0f64;
    let mut last_err = String::new();

    for attempt in 0..max_attempts {
        let model = model_for_plan_attempt(attempt, max_attempts, fallback.as_deref());
        let out = call_llm(state, &messages, model.as_deref(), true, 0.1, None).await?;
        tokens += out.tokens;
        cost += out.cost;

        match serde_json::from_str::<Value>(&out.content)
            .map_err(|e| e.to_string())
            .and_then(|raw| validate_planner_dag(&raw))
        {
            Ok(dag) => return Ok(PlanOutcome { dag, tokens, cost }),
            Err(e) => {
                logd!(
                    "planner attempt {}/{max_attempts}: {e}",
                    attempt + 1
                );
                last_err = e;
            }
        }
    }

    Err(LlmFailure::Llm(format!(
        "Planner failed after {max_attempts} attempt(s) (JSON/schema). Last error: {}",
        if last_err.is_empty() { "unknown error".to_string() } else { last_err }
    )))
}

struct ExpansionOutcome {
    subagents: Vec<SubagentSpec>,
    tokens: i64,
    cost: f64,
}

/// Ask the planner model for additional subagents that depend on `parent_uuid`.
/// Same retry policy as the planner.
async fn generate_subdag_expansion(
    state: &AppState,
    run_goal: &str,
    parent_uuid: &str,
    parent_role: &str,
    parent_output: &str,
    existing_uuids: &HashSet<String>,
) -> Result<ExpansionOutcome, LlmFailure> {
    let mut sorted: Vec<&str> = existing_uuids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let joined = sorted.join(", ");

    let user_blob = format!(
        "Process goal:\n{run_goal}\n\n\
         Parent task UUID: {parent_uuid}\n\
         Parent role: {parent_role}\n\n\
         Existing UUIDs (do not reuse):\n{joined}\n\n\
         Parent output to decompose:\n{}",
        truncate_text_to_tokens(parent_output.trim(), subdag_parent_output_max_tokens())
    );
    let messages =
        [("system", subdag_system_prompt(parent_uuid)), ("user", user_blob)];

    let max_attempts = plan_max_attempts();
    let fallback = planner_fallback_model();
    let mut tokens = 0i64;
    let mut cost = 0.0f64;
    let mut last_err = String::new();

    for attempt in 0..max_attempts {
        let model = model_for_plan_attempt(attempt, max_attempts, fallback.as_deref());
        let out = call_llm(state, &messages, model.as_deref(), true, 0.1, None).await?;
        tokens += out.tokens;
        cost += out.cost;

        match serde_json::from_str::<Value>(&out.content)
            .map_err(|e| e.to_string())
            .and_then(|raw| parse_expansion(&raw, parent_uuid, existing_uuids))
        {
            Ok(subagents) => return Ok(ExpansionOutcome { subagents, tokens, cost }),
            Err(e) => {
                logd!(
                    "sub-decomposition attempt {}/{max_attempts}: {e}",
                    attempt + 1
                );
                last_err = e;
            }
        }
    }

    Err(LlmFailure::Llm(format!(
        "Sub-decomposition planner failed after {max_attempts} attempt(s) (JSON/schema). \
         Last error: {}",
        if last_err.is_empty() { "unknown error".to_string() } else { last_err }
    )))
}

/// `SubagentsOnly.model_validate` plus the two extra rules the expansion adds.
/// Pure, so the rules are testable without an LLM.
fn parse_expansion(
    raw: &Value,
    parent_uuid: &str,
    existing_uuids: &HashSet<String>,
) -> Result<Vec<SubagentSpec>, String> {
    let items = match raw.get("subagents") {
        Some(Value::Array(items)) if !items.is_empty() => items,
        Some(Value::Array(_)) => {
            return Err(
                "1 validation error for SubagentsOnly\nsubagents\n  \
                 List should have at least 1 item after validation, not 0"
                    .to_string(),
            )
        }
        _ => {
            return Err(
                "1 validation error for SubagentsOnly\nsubagents\n  Field required".to_string()
            )
        }
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let spec = validate_subagent(item)?;
        if existing_uuids.contains(&spec.client_uuid) {
            return Err(format!(
                "Sub-decomposition reused existing client_uuid: {}",
                py_repr(&spec.client_uuid)
            ));
        }
        if !spec.dependencies.iter().any(|d| d == parent_uuid) {
            return Err(format!(
                "Sub-decomposition subagent {} must depend on parent {}",
                py_repr(&spec.client_uuid),
                py_repr(parent_uuid)
            ));
        }
        out.push(spec);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Failure text
// ---------------------------------------------------------------------------

/// `_truncate_reason`: strip, then cut to `max_len` **characters** with an
/// ellipsis. `process.failure_reason` is on the wire, so the cut is by char
/// like Python's, not by byte.
fn truncate_reason(msg: &str, max_len: usize) -> String {
    let msg = msg.trim();
    if msg.chars().count() <= max_len {
        return msg.to_string();
    }
    let kept: String = msg.chars().take(max_len.saturating_sub(3)).collect();
    format!("{kept}...")
}

fn truncate_reason_default(msg: &str) -> String {
    truncate_reason(msg, 2048)
}

/// `_task_failure_debug_json`. A derived `Serialize` keeps `json.dumps`'s key
/// order (`serde_json::Map` would sort), and `ensure_ascii=False` matches the
/// call site.
#[derive(Serialize)]
struct FailureDebug<'a> {
    source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    exception_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

fn task_failure_debug_json(failure: &LlmFailure) -> String {
    let message = failure.to_string();
    let body = FailureDebug {
        source: failure.source(),
        exception_type: Some(failure.kind()),
        message: Some(&message),
    };
    let raw = python_json(&body, false);
    if raw.chars().count() <= 16000 {
        raw
    } else {
        truncate_reason(&raw, 16000)
    }
}

/// Extra user message when re-running after `request_changes`.
fn revision_user_preamble(draft_output: Option<&str>, review_feedback: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(draft) = draft_output.filter(|d| !d.is_empty()) {
        parts.push(format!("Previous attempt:\n{draft}"));
    }
    if let Some(feedback) = review_feedback.filter(|f| !f.is_empty()) {
        parts.push(format!("Reviewer feedback:\n{feedback}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("{}\n\nRevise your output to address the feedback above.\n\n", parts.join("\n\n---\n"))
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, FromRow)]
struct ProcessRow {
    goal: String,
    status: String,
    dag_json: Option<String>,
    tool_invocations_used: i64,
    token_id: Option<i64>,
}

const PROCESS_COLUMNS: &str = "goal, status, dag_json, tool_invocations_used, token_id";

#[derive(Debug, FromRow)]
struct TaskRow {
    id: i64,
    client_uuid: String,
    parent_client_uuid: Option<String>,
    role: String,
    system_prompt: String,
    instructions: String,
    llm_model: Option<String>,
    dependencies_json: String,
    /// `INTEGER` on both backends; `i64` for the same reason as
    /// [`crate::processes`]'s copy. Compared against 0 at its three use sites.
    requires_review: i64,
    draft_output: Option<String>,
    review_feedback: Option<String>,
}

const TASK_COLUMNS: &str = "id, client_uuid, parent_client_uuid, role, system_prompt, \
     instructions, llm_model, dependencies_json, requires_review, draft_output, review_feedback";

/// `dependencies_json` is written by this process and by Python, both as a JSON
/// array of strings. A value that will not parse is a corrupt row, and treating
/// it as "no dependencies" is the reading that lets the wave make progress.
fn parse_dependencies(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_else(|_| {
        logd!("unreadable dependencies_json: {raw}");
        Vec::new()
    })
}

async fn load_process(state: &AppState, process_id: i64) -> Result<Option<ProcessRow>, sqlx::Error> {
    sqlx::query_as(&crate::db::sql(&format!("SELECT {PROCESS_COLUMNS} FROM process WHERE id = ?"), state.backend))
        .bind(process_id)
        .fetch_optional(&state.any)
        .await
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// `services/event_log_service.append_event`.
async fn append_event(
    state: &AppState,
    process_id: i64,
    task_id: Option<i64>,
    event_type: &str,
    content: &str,
) {
    let result = sqlx::query(&crate::db::sql(
        "INSERT INTO eventlog (process_id, task_id, event_type, content, created_at) \
         VALUES (?, ?, ?, ?, ?)", state.backend)
    )
    .bind(process_id)
    .bind(task_id)
    .bind(event_type)
    .bind(content)
    .bind(sql_now())
    .execute(&state.any)
    .await;
    if let Err(e) = result {
        logd!("event log write failed for process {process_id}: {e}");
    }
}

/// `api_tokens/usage_tracking.record_api_token_usage`, as `SET x = x + 1`.
///
/// Python reads the row, mutates the object and commits — a lost update whenever
/// two writers overlap. Rust's increment is atomic, which does **not** rescue
/// the pair — while Python could still reach it. **That window is closed as of
/// 2026-08-07**: coder moved to Rust, playground was deleted rather than ported,
/// and the assistant's call site passes a literal `None`. The four remaining
/// Python importers all sit behind routes Rust now owns, so nothing on that side
/// increments these rows any more.
///
/// Master-key callers have no `token_id` and are not tracked at all.
pub(crate) async fn record_api_token_usage(
    state: &AppState,
    token_id: Option<i64>,
    tokens: i64,
    cost: f64,
    is_error: bool,
) {
    let Some(token_id) = token_id else { return };
    let errors = i64::from(is_error);
    let today = chrono::Utc::now().naive_utc().format("%Y-%m-%d").to_string();

    let updated = sqlx::query(&crate::db::sql(
        "UPDATE api_token_usage_daily \
         SET request_count = request_count + 1, error_count = error_count + ?, \
             total_tokens = total_tokens + ?, total_cost = total_cost + ? \
         WHERE token_id = ? AND usage_date = ?", state.backend)
    )
    .bind(errors)
    .bind(tokens)
    .bind(cost)
    .bind(token_id)
    .bind(&today)
    .execute(&state.any)
    .await;

    match updated {
        Ok(result) if result.rows_affected() == 0 => {
            let insert = sqlx::query(&crate::db::sql(
                "INSERT INTO api_token_usage_daily \
                 (token_id, usage_date, request_count, error_count, total_tokens, total_cost) \
                 VALUES (?, ?, 1, ?, ?, ?)", state.backend)
            )
            .bind(token_id)
            .bind(&today)
            .bind(errors)
            .bind(tokens)
            .bind(cost)
            .execute(&state.any)
            .await;
            if let Err(e) = insert {
                logd!("daily usage insert failed for token {token_id}: {e}");
            }
        }
        Err(e) => logd!("daily usage update failed for token {token_id}: {e}"),
        Ok(_) => {}
    }

    let lifetime = sqlx::query(&crate::db::sql(
        "UPDATE api_tokens \
         SET total_requests = total_requests + 1, total_errors = total_errors + ?, \
             total_tokens = total_tokens + ?, total_cost = total_cost + ? \
         WHERE id = ?", state.backend)
    )
    .bind(errors)
    .bind(tokens)
    .bind(cost)
    .bind(token_id)
    .execute(&state.any)
    .await;
    if let Err(e) = lifetime {
        logd!("token usage update failed for token {token_id}: {e}");
    }
}

/// `mark_process_planning` and auto-approve's `run.status = "approved"`: status
/// **only**, leaving `failure_reason` where it was. Two of the service functions
/// write it and two do not, and that difference shows up in `GET /processes/{id}`.
///
/// `updated_at` is untouched throughout: `Process` has a `default_factory` and
/// no `onupdate`, so Python does not move it either.
async fn set_process_status_only(
    state: &AppState,
    process_id: i64,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(&crate::db::sql("UPDATE process SET status = ? WHERE id = ?", state.backend))
        .bind(status)
        .bind(process_id)
        .execute(&state.any)
        .await?;
    Ok(())
}

/// The `process_runtime_service` shape: status plus an explicit
/// `failure_reason` (`None` clears it, which is what the running / paused /
/// completed transitions all do).
async fn set_process_status(
    state: &AppState,
    process_id: i64,
    status: &str,
    failure_reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&crate::db::sql("UPDATE process SET status = ?, failure_reason = ? WHERE id = ?", state.backend))
        .bind(status)
        .bind(failure_reason)
        .bind(process_id)
        .execute(&state.any)
        .await?;
    Ok(())
}

async fn fail_process(state: &AppState, process_id: i64, reason: &str) -> Result<(), sqlx::Error> {
    set_process_status(state, process_id, "failed", Some(reason)).await
}

/// `process_approval.apply_validated_planner_to_process`: replace every
/// `TaskNode` and set the canonical `dag_json`.
///
/// Shared with `processes::approve` and `processes::retry`, which materialise a
/// caller-submitted DAG through exactly this path — the auto-approve branch here
/// and the human-approve branch there have to write the same rows and the same
/// canonical text, so there is one function rather than two that agree today.
pub(crate) async fn apply_validated_planner_to_process(
    state: &AppState,
    process_id: i64,
    validated: &PlannerDag,
) -> Result<(), sqlx::Error> {
    // `ensure_ascii=False` here and `True` in `apply_planner_success`. Both are
    // stored as-is and echoed back by `GET /processes/{id}`.
    let canonical = planner_dag_to_json(validated, false);
    let mut tx = state.any.begin().await?;
    // Insert **before** delete, which reads backwards from `process_approval.py`
    // and is exactly what it does: SQLAlchemy's unit of work flushes INSERTs
    // ahead of DELETEs, so the replacement rows are numbered while the old ones
    // are still there. Deleting first empties the table, and SQLite then restarts
    // `rowid` at 1 — so a re-materialised DAG reused task ids a client may still
    // hold, and `/tasks/1/retry` would silently address a *different* task where
    // Python 404s. Found by cross-rendering a retry through both servers: the
    // same twelve rows, ids 1-12 here against 55-66 there.
    let old_max: i64 =
        sqlx::query_scalar(&crate::db::sql("SELECT COALESCE(MAX(id), 0) FROM tasknode WHERE process_id = ?", state.backend))
            .bind(process_id)
            .fetch_one(&mut *tx)
            .await?;
    for agent in &validated.subagents {
        insert_task_node(&mut tx, state.backend, process_id, agent, None).await?;
    }
    sqlx::query(&crate::db::sql("DELETE FROM tasknode WHERE process_id = ? AND id <= ?", state.backend))
        .bind(process_id)
        .bind(old_max)
        .execute(&mut *tx)
        .await?;
    sqlx::query(&crate::db::sql("UPDATE process SET dag_json = ? WHERE id = ?", state.backend))
        .bind(&canonical)
        .bind(process_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

async fn insert_task_node(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: crate::db::Backend,
    process_id: i64,
    agent: &SubagentSpec,
    parent_client_uuid: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&crate::db::sql(
        "INSERT INTO tasknode \
         (process_id, client_uuid, parent_client_uuid, role, system_prompt, instructions, \
          llm_model, dependencies_json, status, requires_review, revision_count, tokens_used) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, 0, 0)", backend)
    )
    .bind(process_id)
    .bind(&agent.client_uuid)
    .bind(parent_client_uuid)
    .bind(&agent.role)
    .bind(&agent.system_prompt)
    .bind(&agent.instructions)
    .bind(agent.llm_model.as_deref())
    // `json.dumps(list)` — the space after the comma is echoed raw by
    // `GET /processes/{id}` as `dependencies_json`.
    .bind(python_json(&agent.dependencies, true))
    .bind(agent.requires_review)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Review assignment (`services/review_assignment_service.py`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
struct ReviewTask {
    id: i64,
    client_uuid: String,
    status: String,
    dependencies_json: String,
    reviewer_client_uuid: Option<String>,
}

impl ReviewTask {
    fn dependencies(&self) -> Vec<String> {
        parse_dependencies(&self.dependencies_json)
    }
}

fn deps_all_completed(task: &ReviewTask, by_uuid: &HashMap<&str, &ReviewTask>) -> bool {
    task.dependencies()
        .iter()
        .all(|dep| by_uuid.get(dep.as_str()).is_some_and(|row| row.status == "completed"))
}

/// A peer may review when it is done, or when it is idle in the DAG — pending
/// with dependencies that are not all satisfied, so it will not be scheduled in
/// this wave anyway.
fn can_serve_as_reviewer(task: &ReviewTask, by_uuid: &HashMap<&str, &ReviewTask>) -> bool {
    if task.status == "completed" {
        return true;
    }
    if task.status != "pending" {
        return false;
    }
    if task.dependencies().is_empty() {
        return false;
    }
    !deps_all_completed(task, by_uuid)
}

fn role_word_overlap(r1: &str, r2: &str) -> usize {
    let words = |role: &str| -> HashSet<String> {
        role.to_lowercase().replace('-', " ").split_whitespace().map(str::to_string).collect()
    };
    let (a, b) = (words(r1), words(r2));
    a.intersection(&b).count()
}

fn pick_reviewer_client_uuid(
    subject: &ReviewTask,
    subagents: &[SubagentSpec],
    by_uuid: &HashMap<&str, &ReviewTask>,
) -> Option<String> {
    let author = subject.client_uuid.as_str();
    let author_spec = subagents.iter().find(|a| a.client_uuid == author)?;
    let downstream: HashSet<&str> = subagents
        .iter()
        .filter(|a| a.dependencies.iter().any(|d| d == author))
        .map(|a| a.client_uuid.as_str())
        .collect();
    let upstream: HashSet<&str> = author_spec.dependencies.iter().map(String::as_str).collect();

    let mut scored: Vec<(i64, &str)> = Vec::new();
    for candidate in subagents {
        let cu = candidate.client_uuid.as_str();
        if cu == author {
            continue;
        }
        let Some(row) = by_uuid.get(cu) else { continue };
        if !can_serve_as_reviewer(row, by_uuid) {
            continue;
        }
        let mut score = 0i64;
        if downstream.contains(cu) {
            score += 100;
        }
        // Always fires together with the 100 above — same predicate, both kept
        // because the totals are what a reader compares against Python's.
        if candidate.dependencies.iter().any(|d| d == author) {
            score += 80;
        }
        if upstream.contains(cu) {
            score += 40;
        }
        score += role_word_overlap(&author_spec.role, &candidate.role) as i64;
        scored.push((score, cu));
    }

    // `key=lambda x: (-score, cu)`: best score, then the lowest uuid.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored.first().map(|(_, cu)| cu.to_string())
}

/// The rows whose `reviewer_client_uuid` has to change, and what to. Pure, so
/// the picker is testable without a database.
fn compute_review_assignments(
    tasks: &[ReviewTask],
    subagents: &[SubagentSpec],
) -> Vec<(i64, Option<String>)> {
    let by_uuid: HashMap<&str, &ReviewTask> =
        tasks.iter().map(|t| (t.client_uuid.as_str(), t)).collect();
    let mut changes = Vec::new();

    for task in tasks {
        if task.status != "awaiting_review" {
            continue;
        }
        // An empty string is falsy in Python, so it reads as "no reviewer".
        let mut current = task.reviewer_client_uuid.clone().filter(|r| !r.is_empty());
        let mut changed = false;

        let stale = current.as_deref().is_some_and(|reviewer| {
            !by_uuid.get(reviewer).is_some_and(|row| can_serve_as_reviewer(row, &by_uuid))
        });
        if stale {
            current = None;
            changed = true;
        }
        if current.is_none() {
            if let Some(pick) = pick_reviewer_client_uuid(task, subagents, &by_uuid) {
                current = Some(pick);
                changed = true;
            }
        }
        if changed {
            changes.push((task.id, current));
        }
    }
    changes
}

async fn sync_review_assignments(state: &AppState, process_id: i64) -> Result<(), sqlx::Error> {
    let Some(process) = load_process(state, process_id).await? else { return Ok(()) };
    let Some(dag_json) = process.dag_json.filter(|d| !d.is_empty()) else { return Ok(()) };
    let Ok(planner) = serde_json::from_str::<Value>(&dag_json)
        .map_err(|e| e.to_string())
        .and_then(|raw| validate_planner_dag(&raw))
    else {
        // Python swallows both the JSON error and the ValueError here.
        return Ok(());
    };

    let tasks: Vec<ReviewTask> = sqlx::query_as(&crate::db::sql(
        "SELECT id, client_uuid, status, dependencies_json, reviewer_client_uuid \
         FROM tasknode WHERE process_id = ?", state.backend)
    )
    .bind(process_id)
    .fetch_all(&state.any)
    .await?;

    for (task_id, reviewer) in compute_review_assignments(&tasks, &planner.subagents) {
        sqlx::query(&crate::db::sql("UPDATE tasknode SET reviewer_client_uuid = ? WHERE id = ?", state.backend))
            .bind(reviewer)
            .bind(task_id)
            .execute(&state.any)
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The wave (`services/dag_runtime_service.py`) — pure
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct PendingTask {
    id: i64,
    client_uuid: String,
    dependencies: Vec<String>,
}

#[derive(Debug, Default)]
struct DagSnapshot {
    pending_tasks: Vec<PendingTask>,
    awaiting_review_exists: bool,
    completed_uuids: HashSet<String>,
    /// Status of every task in the process, by `client_uuid`. Only read to
    /// explain a deadlock — the wave itself needs nothing but the three fields
    /// above.
    status_by_uuid: HashMap<String, String>,
}

/// What the loop does with one snapshot. Naming the four outcomes is what makes
/// the deadlock and review-gate branches testable without a database.
#[derive(Debug, PartialEq)]
enum Wave {
    /// Nothing pending and no gate open: the process is done.
    Complete,
    /// Nothing runnable, but a human is holding a task.
    PauseForReview,
    /// Pending tasks and none of them runnable, with the blockers named.
    Deadlock(String),
    /// Task ids to run now, FIFO by id and capped by the concurrency limit.
    Run(Vec<i64>),
}

fn select_ready_task_ids(
    pending_tasks: &[PendingTask],
    completed_uuids: &HashSet<String>,
    max_concurrent: Option<usize>,
) -> Vec<i64> {
    let mut ready: Vec<i64> = pending_tasks
        .iter()
        .filter(|task| task.dependencies.iter().all(|dep| completed_uuids.contains(dep)))
        .map(|task| task.id)
        .collect();
    // Ascending id: free capacity picks the next id, like a worker pool.
    ready.sort_unstable();
    if let Some(cap) = max_concurrent {
        ready.truncate(cap);
    }
    ready
}

fn plan_wave(snapshot: &DagSnapshot, max_concurrent: Option<usize>) -> Wave {
    if snapshot.pending_tasks.is_empty() {
        return if snapshot.awaiting_review_exists { Wave::PauseForReview } else { Wave::Complete };
    }
    let ready =
        select_ready_task_ids(&snapshot.pending_tasks, &snapshot.completed_uuids, max_concurrent);
    if ready.is_empty() {
        // A dependent stuck behind an `awaiting_review` upstream is a pause, not
        // a deadlock — that distinction is the whole point of checking the gate
        // before declaring one.
        return if snapshot.awaiting_review_exists {
            Wave::PauseForReview
        } else {
            Wave::Deadlock(deadlock_reason(snapshot))
        };
    }
    Wave::Run(ready)
}

/// Name the blockers. "cycle or unsatisfied dependencies" is true and useless:
/// the usual cause is one failed upstream task the user can retry, and the user
/// cannot tell that from a graph problem without being told which.
fn deadlock_reason(snapshot: &DagSnapshot) -> String {
    // Sorted and de-duplicated so the message is stable across runs.
    let mut failed: BTreeSet<&str> = BTreeSet::new();
    let mut unknown: BTreeSet<&str> = BTreeSet::new();
    let mut waiting: BTreeSet<&str> = BTreeSet::new();

    for task in &snapshot.pending_tasks {
        for dep in &task.dependencies {
            if snapshot.completed_uuids.contains(dep) {
                continue;
            }
            match snapshot.status_by_uuid.get(dep.as_str()).map(String::as_str) {
                Some("failed") => failed.insert(dep.as_str()),
                // A dep that is itself pending here can never start: this task
                // and that one are in a cycle (or behind one).
                Some(_) => waiting.insert(task.client_uuid.as_str()),
                None => unknown.insert(dep.as_str()),
            };
        }
    }

    let list = |set: BTreeSet<&str>| set.into_iter().collect::<Vec<_>>().join(", ");
    let mut parts: Vec<String> = Vec::new();
    if !failed.is_empty() {
        parts.push(format!(
            "blocked by failed task(s) {} — retry those tasks, or retry the process",
            list(failed)
        ));
    }
    if !unknown.is_empty() {
        parts.push(format!("dependencies on task(s) that do not exist: {}", list(unknown)));
    }
    if !waiting.is_empty() {
        parts.push(format!("cyclic dependencies among task(s) {}", list(waiting)));
    }
    if parts.is_empty() {
        // No pending task has an unmet dep, yet none was selected: only a
        // concurrency cap of zero can do that.
        parts.push("pending tasks with no runnable step".to_string());
    }
    format!("DAG deadlock: {}", parts.join("; "))
}

async fn load_dag_task_snapshot(
    state: &AppState,
    process_id: i64,
) -> Result<DagSnapshot, sqlx::Error> {
    // One read of every task rather than three status-filtered ones: the
    // deadlock message needs the statuses the other two threw away.
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(&crate::db::sql(
        "SELECT id, client_uuid, status, dependencies_json FROM tasknode WHERE process_id = ?", state.backend)
    )
    .bind(process_id)
    .fetch_all(&state.any)
    .await?;

    let mut snapshot = DagSnapshot::default();
    for (id, client_uuid, status, deps) in rows {
        match status.as_str() {
            "pending" => snapshot.pending_tasks.push(PendingTask {
                id,
                client_uuid: client_uuid.clone(),
                dependencies: parse_dependencies(&deps),
            }),
            "awaiting_review" => snapshot.awaiting_review_exists = true,
            "completed" => {
                snapshot.completed_uuids.insert(client_uuid.clone());
            }
            _ => {}
        }
        snapshot.status_by_uuid.insert(client_uuid, status);
    }
    Ok(snapshot)
}

// ---------------------------------------------------------------------------
// Sub-DAG expansion gates — pure
// ---------------------------------------------------------------------------

struct ExpansionGate<'a> {
    process_status: &'a str,
    expansions_used: i64,
    new_tasks_added: i64,
    max_expansions: i64,
    max_new_tasks: i64,
    spec: Option<&'a SubagentSpec>,
    parent_depth: i64,
    max_depth: Option<i64>,
}

/// Every early return in `_maybe_expand_subdag_after_success` before the LLM
/// call, in Python's order.
fn expansion_allowed(gate: &ExpansionGate) -> bool {
    if gate.max_expansions <= 0 || gate.max_new_tasks <= 0 {
        return false;
    }
    if gate.process_status != "running" {
        return false;
    }
    if gate.expansions_used >= gate.max_expansions {
        return false;
    }
    if gate.new_tasks_added >= gate.max_new_tasks {
        return false;
    }
    let Some(spec) = gate.spec else { return false };
    if !spec.subdecompose || spec.requires_review {
        return false;
    }
    if let Some(max_depth) = gate.max_depth {
        if gate.parent_depth + 1 > max_depth {
            return false;
        }
    }
    true
}

/// `remaining_slots` is computed **before** the LLM call and checked **after**
/// it, which is Python's order and therefore this one's: an expansion that runs
/// out of slots while the model is thinking still pays for the call.
fn cap_expansion(mut specs: Vec<SubagentSpec>, remaining_slots: i64) -> Option<Vec<SubagentSpec>> {
    if remaining_slots <= 0 {
        return None;
    }
    specs.truncate(remaining_slots as usize);
    Some(specs)
}

// ---------------------------------------------------------------------------
// Team context (`team_schema.team_context_from_snapshot_json`)
// ---------------------------------------------------------------------------

/// Number of ancestor edges to a root; a cycle stops the walk where it closes.
fn role_depth(role_id: &str, parent_by_id: &HashMap<&str, Option<&str>>) -> usize {
    let mut depth = 0usize;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut current = Some(role_id);
    while let Some(id) = current {
        if !seen.insert(id) {
            return depth;
        }
        match parent_by_id.get(id).copied().flatten() {
            None => break,
            Some(parent) => {
                depth += 1;
                current = Some(parent);
            }
        }
    }
    depth
}

fn render_team_context_for_planner(
    name: &str,
    description: Option<&str>,
    color: Option<&str>,
    roster: &TeamRoster,
) -> String {
    let mut lines = vec![format!("Team template: {name}")];
    if let Some(description) = description.map(str::trim).filter(|d| !d.is_empty()) {
        lines.push(format!("Team description: {description}"));
    }
    if let Some(color) = color.map(str::trim).filter(|c| !c.is_empty()) {
        lines.push(format!("Team color (UI hint): {color}"));
    }
    lines.push(
        "Preferred team roster (map subagent `role` names and responsibilities to these where \
         sensible):"
            .to_string(),
    );

    let parent_by_id: HashMap<&str, Option<&str>> =
        roster.roles.iter().map(|r| (r.id.as_str(), r.parent_id.as_deref())).collect();
    let mut ordered: Vec<_> = roster.roles.iter().collect();
    // Stable, like Python's `sorted`.
    ordered.sort_by_key(|r| (role_depth(&r.id, &parent_by_id), r.name.to_lowercase()));

    for role in ordered {
        let depth = role_depth(&role.id, &parent_by_id);
        let mut line = format!("{}- {} (id={})", "  ".repeat(depth), role.name, role.id);
        if !role.description.trim().is_empty() {
            line.push_str(&format!(": {}", role.description.trim()));
        }
        if role.modality != "text" {
            line.push_str(&format!(" [modality: {}]", role.modality));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// The planner's `team_context` argument, rebuilt from the snapshot stored on
/// the process.
///
/// ponytail: `parse_team_roster_dict` runs pydantic's graph validators (dup ids,
/// unknown parent, cycles) and returns `None` when they fail; this only
/// deserializes, so a roster Python would reject renders here instead. Same
/// looseness `teams::parse_roster` already has on the read path, and the output
/// is prompt text.
pub fn team_context_from_snapshot_json(snapshot_json: Option<&str>) -> Option<String> {
    let raw = snapshot_json.map(str::trim).filter(|s| !s.is_empty())?;
    let data: Value = serde_json::from_str(raw).ok()?;
    let data = data.as_object()?;

    let name = data.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let name = if name.is_empty() { "Team" } else { name };
    let description = data.get("description").and_then(Value::as_str);
    let color = data.get("color").and_then(Value::as_str);
    let roster: TeamRoster = serde_json::from_value(data.get("roster")?.clone()).ok()?;

    Some(render_team_context_for_planner(name, description, color, &roster))
}

// ---------------------------------------------------------------------------
// The executor
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Subdecomp {
    expansions_used: i64,
    new_tasks_added: i64,
    /// client_uuid → depth of tasks introduced by expansion (planner tasks are 0).
    uuid_depth: HashMap<String, i64>,
}

struct Executor {
    state: Arc<AppState>,
    process_id: i64,
    auto_approve: bool,
    /// `futures::lock::Mutex`, not `std`: it is held across the expansion's LLM
    /// call, exactly as Python's `asyncio.Lock` is, and a std guard across an
    /// `.await` makes the future `!Send`. (`tokio::sync` is not compiled in —
    /// this crate does not enable tokio's `sync` feature.)
    subdecomp: futures::lock::Mutex<Subdecomp>,
}

impl Executor {
    fn new(state: Arc<AppState>, process_id: i64, auto_approve: bool) -> Arc<Self> {
        Arc::new(Self {
            state,
            process_id,
            auto_approve,
            subdecomp: futures::lock::Mutex::new(Subdecomp::default()),
        })
    }

    fn should_auto_approve(&self) -> bool {
        self.auto_approve || env_auto_approve()
    }

    async fn log(&self, event_type: &str, content: &str, task_id: Option<i64>) {
        append_event(&self.state, self.process_id, task_id, event_type, content).await;
    }

    // -- planning ----------------------------------------------------------

    async fn plan(self: Arc<Self>, goal: String, team_context: Option<String>) {
        // `mark_process_planning` writes the status and nothing else, so a
        // previous run's `failure_reason` stays visible while re-planning.
        if let Err(e) = set_process_status_only(&self.state, self.process_id, "planning").await {
            logd!(
                "process {} could not be marked planning: {e}",
                self.process_id
            );
            return;
        }
        self.log("status_change", "Process status updated to planning", None).await;

        let timeout = plan_timeout_seconds();
        let attempt = generate_planner_dag(&self.state, &goal, team_context.as_deref());
        let outcome = match timeout {
            Some(seconds) => {
                match tokio::time::timeout(duration_from_secs_f64(seconds), attempt).await {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        let reason = "Planning exceeded AGENT_PLATFORM_PLAN_TIMEOUT_SECONDS";
                        self.apply_planner_failure(reason).await;
                        self.log("error", &format!("Planning failed: {reason}"), None).await;
                        return;
                    }
                }
            }
            None => attempt.await,
        };

        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(e) => {
                let reason = truncate_reason_default(&format!("Planning failed: {e}"));
                self.apply_planner_failure(&reason).await;
                self.log("error", &format!("Planning failed: {e}"), None).await;
                return;
            }
        };

        if let Err(e) = self.apply_planner_success(&outcome).await {
            logd!("planner result could not be stored: {e}");
            let reason = "Planning failed: unexpected error";
            self.apply_planner_failure(reason).await;
            self.log("error", reason, None).await;
            return;
        }
        self.log("status_change", "Process requires approval to execute DAG", None).await;

        if self.should_auto_approve() {
            self.auto_approve_and_execute().await;
        }
    }

    /// `services/planner_runtime_service.apply_planner_success`.
    async fn apply_planner_success(&self, outcome: &PlanOutcome) -> Result<(), sqlx::Error> {
        let Some(process) = load_process(&self.state, self.process_id).await? else {
            return Err(sqlx::Error::RowNotFound);
        };
        let mut tx = self.state.any.begin().await?;
        sqlx::query(&crate::db::sql(
            "UPDATE process SET dag_json = ?, total_tokens = total_tokens + ?, \
             total_cost = total_cost + ?, failure_reason = NULL, status = 'approval_required' \
             WHERE id = ?", self.state.backend)
        )
        // `json.dumps(dag)` with the default `ensure_ascii=True`.
        .bind(planner_dag_to_json(&outcome.dag, true))
        .bind(outcome.tokens)
        .bind(outcome.cost)
        .bind(self.process_id)
        .execute(&mut *tx)
        .await?;
        for agent in &outcome.dag.subagents {
            insert_task_node(&mut tx, self.state.backend, self.process_id, agent, None).await?;
        }
        tx.commit().await?;

        record_api_token_usage(&self.state, process.token_id, outcome.tokens, outcome.cost, false)
            .await;
        Ok(())
    }

    /// `services/planner_runtime_service.apply_planner_failure`.
    async fn apply_planner_failure(&self, reason: &str) {
        let token_id = load_process(&self.state, self.process_id)
            .await
            .ok()
            .flatten()
            .and_then(|p| p.token_id);
        if let Err(e) = fail_process(&self.state, self.process_id, reason).await {
            logd!("process {} could not be failed: {e}", self.process_id);
        }
        record_api_token_usage(&self.state, token_id, 0, 0.0, true).await;
    }

    async fn auto_approve_and_execute(self: Arc<Self>) {
        let Ok(Some(process)) = load_process(&self.state, self.process_id).await else { return };
        if process.status != "approval_required" {
            return;
        }
        let Some(dag_json) = process.dag_json.filter(|d| !d.is_empty()) else { return };

        let validated = match serde_json::from_str::<Value>(&dag_json)
            .map_err(|e| e.to_string())
            .and_then(|raw| validate_planner_dag(&raw))
        {
            Ok(validated) => validated,
            Err(e) => {
                logd!("auto-approve skipped (invalid DAG): {e}");
                self.log("error", &format!("Auto-approve skipped: {e}"), None).await;
                return;
            }
        };

        if let Err(e) =
            apply_validated_planner_to_process(&self.state, self.process_id, &validated).await
        {
            logd!("auto-approve could not persist the DAG: {e}");
            return;
        }
        // Status only: Python assigns `run.status` here and never touches
        // `failure_reason`, so a retry after a failure keeps the old text until
        // `execute_dag` clears it.
        if let Err(e) = set_process_status_only(&self.state, self.process_id, "approved").await {
            logd!("auto-approve could not mark approved: {e}");
            return;
        }
        self.log("status_change", "Process auto-approved; scheduling execution", None).await;
        self.execute_dag().await;
    }

    // -- the wave loop -----------------------------------------------------

    async fn execute_dag(self: Arc<Self>) {
        let deadline = run_max_seconds().map(|s| Instant::now() + duration_from_secs_f64(s));

        let awaiting_left = match sqlx::query_scalar::<_, i64>(&crate::db::sql(
            "SELECT CAST(id AS BIGINT) FROM tasknode WHERE process_id = ? AND status = 'awaiting_review' LIMIT 1",
            self.state.backend,
        )
        )
        .bind(self.process_id)
        .fetch_optional(&self.state.any)
        .await
        {
            Ok(row) => row.is_some(),
            Err(e) => {
                logd!("execute_dag could not read tasks: {e}");
                return;
            }
        };
        let status = if awaiting_left { "task_review_required" } else { "running" };
        if let Err(e) = set_process_status(&self.state, self.process_id, status, None).await {
            logd!("execute_dag could not set status: {e}");
            return;
        }
        self.log(
            "status_change",
            if awaiting_left {
                "Process status updated to task_review_required"
            } else {
                "Process status updated to running"
            },
            None,
        )
        .await;

        loop {
            // Cancellation and pause are DB-mediated: `/cancel` and `/sync`
            // write a status and this read is where the loop notices.
            if let Err(e) = sync_review_assignments(&self.state, self.process_id).await {
                logd!("review assignment sync failed: {e}");
                return;
            }
            let Ok(Some(process)) = load_process(&self.state, self.process_id).await else {
                logd!("process {} vanished mid-run", self.process_id);
                return;
            };
            if process.status == "cancelled" {
                self.log("status_change", "Process stopped (cancelled)", None).await;
                return;
            }
            if process.status == "failed" {
                return;
            }
            if deadline.is_some_and(|deadline| Instant::now() > deadline) {
                let reason = "Process exceeded execution budget (AGENT_PLATFORM_RUN_MAX_SECONDS)";
                let _ = fail_process(&self.state, self.process_id, reason).await;
                self.log("error", reason, None).await;
                return;
            }

            let snapshot = match load_dag_task_snapshot(&self.state, self.process_id).await {
                Ok(snapshot) => snapshot,
                Err(e) => {
                    logd!("execute_dag snapshot failed: {e}");
                    return;
                }
            };

            match plan_wave(&snapshot, max_concurrent_tasks()) {
                Wave::PauseForReview => {
                    let _ = set_process_status(
                        &self.state,
                        self.process_id,
                        "task_review_required",
                        None,
                    )
                    .await;
                    self.log("status_change", "Process paused for task review", None).await;
                    return;
                }
                Wave::Complete => {
                    let _ =
                        set_process_status(&self.state, self.process_id, "completed", None).await;
                    self.log("status_change", "Process execution fully completed", None).await;
                    return;
                }
                Wave::Deadlock(reason) => {
                    let _ = fail_process(&self.state, self.process_id, &reason).await;
                    self.log("error", &reason, None).await;
                    return;
                }
                Wave::Run(task_ids) => {
                    // `asyncio.gather` over the same ids in the same order.
                    let mut wave: JoinSet<()> = JoinSet::new();
                    for task_id in task_ids {
                        let executor = Arc::clone(&self);
                        wave.spawn(async move { executor.execute_task(task_id).await });
                    }
                    while let Some(joined) = wave.join_next().await {
                        if let Err(e) = joined {
                            logd!("task in wave crashed: {e}");
                        }
                    }
                }
            }
        }
    }

    // -- one task ----------------------------------------------------------

    async fn execute_task(self: Arc<Self>, task_id: i64) {
        let inputs = match self.load_task_execution_inputs(task_id).await {
            Ok(Some(inputs)) => inputs,
            Ok(None) => return,
            Err(e) => {
                logd!("task {task_id} could not be started: {e}");
                return;
            }
        };
        let TaskInputs { task, deps_texts, system_message, user_message } = inputs;

        let user_message = match self
            .build_user_message_with_context(&task, deps_texts, &system_message, user_message)
            .await
        {
            Ok(message) => message,
            Err(failure) => {
                self.record_task_failure(&task, &failure).await;
                return;
            }
        };

        let messages =
            [("system", system_message.clone()), ("user", user_message)];
        match self.invoke_task_llm(task.llm_model.as_deref(), &messages).await {
            Ok((completion, tool_calls)) => {
                match self.apply_task_success(&task, &completion, tool_calls).await {
                    Ok(needs_expand) => {
                        if needs_expand {
                            self.maybe_expand_subdag_after_success(task_id).await;
                        }
                    }
                    Err(e) => {
                        logd!("task {task_id} result could not be stored: {e}");
                    }
                }
            }
            Err(failure) => self.record_task_failure(&task, &failure).await,
        }
    }

    /// `_load_task_execution_inputs`: flip the task to `running`, log it, and
    /// gather the dependency outputs and prompts.
    async fn load_task_execution_inputs(
        &self,
        task_id: i64,
    ) -> Result<Option<TaskInputs>, sqlx::Error> {
        let Some(task): Option<TaskRow> =
            sqlx::query_as(&crate::db::sql(&format!("SELECT {TASK_COLUMNS} FROM tasknode WHERE id = ?"), self.state.backend))
                .bind(task_id)
                .fetch_optional(&self.state.any)
                .await?
        else {
            return Ok(None);
        };

        sqlx::query(&crate::db::sql(
            "UPDATE tasknode SET status = 'running', started_at = ?, failure_debug_json = NULL \
             WHERE id = ?", self.state.backend)
        )
        .bind(sql_now())
        .bind(task_id)
        .execute(&self.state.any)
        .await?;
        self.log(
            "status_change",
            &format!("Task {} started executing", task.client_uuid),
            Some(task.id),
        )
        .await;

        let dependencies = parse_dependencies(&task.dependencies_json);
        let mut deps_texts: Vec<String> = Vec::new();
        if !dependencies.is_empty() {
            let placeholders = vec!["?"; dependencies.len()].join(", ");
            // Bound to a local: `sqlx::query_as` borrows the SQL for the life of
            // the query, and a `&format!(…)` temporary would not outlive the let.
            let sql = format!(
                "SELECT client_uuid, role, output FROM tasknode \
                 WHERE process_id = ? AND client_uuid IN ({placeholders})"
            );
            let mut query =
                sqlx::query_as::<_, (String, String, Option<String>)>(&sql).bind(self.process_id);
            for dep in &dependencies {
                query = query.bind(dep);
            }
            for (client_uuid, role, output) in query.fetch_all(&self.state.any).await? {
                deps_texts.push(format!(
                    "Output from {client_uuid} ({role}):\n{}",
                    output.unwrap_or_default()
                ));
            }
        }

        let system_message = task.system_prompt.clone();
        let mut user_message = task.instructions.clone();
        if let Some(parent_uuid) = task.parent_client_uuid.as_deref().filter(|p| !p.is_empty()) {
            let parent_role: Option<String> = sqlx::query_scalar(&crate::db::sql(
                "SELECT role FROM tasknode WHERE process_id = ? AND client_uuid = ? LIMIT 1", self.state.backend)
            )
            .bind(self.process_id)
            .bind(parent_uuid)
            .fetch_optional(&self.state.any)
            .await?;
            if let Some(parent_role) = parent_role {
                user_message = format!(
                    "This is a subtask spawned after `{parent_role}` ({parent_uuid}) completed. \
                     Deliver one focused outcome.\n\n{user_message}"
                );
            }
        }

        Ok(Some(TaskInputs { task, deps_texts, system_message, user_message }))
    }

    async fn build_user_message_with_context(
        &self,
        task: &TaskRow,
        deps_texts: Vec<String>,
        system_message: &str,
        user_message: String,
    ) -> Result<String, LlmFailure> {
        // The revision preamble goes in front of the parent preamble, which
        // `load_task_execution_inputs` already prepended.
        let preamble = revision_user_preamble(
            task.draft_output.as_deref(),
            task.review_feedback.as_deref(),
        );
        let mut user_message = format!("{preamble}{user_message}");

        if deps_texts.is_empty() {
            return Ok(user_message);
        }
        let budget = dependency_context_token_budget(system_message, &user_message);
        let mut condensed = Vec::with_capacity(deps_texts.len());
        for chunk in &deps_texts {
            condensed.push(
                maybe_condense_text_for_context(&self.state, chunk, task.llm_model.as_deref())
                    .await?,
            );
        }
        let fitted = fit_dependency_outputs_to_budget(&condensed, budget);
        user_message.push_str("\n\nContext from previous steps:\n");
        user_message.push_str(&fitted.join("\n---\n"));
        Ok(user_message)
    }

    /// `_invoke_task_llm`. The tool branch is refused rather than silently
    /// downgraded — see the module docs.
    async fn invoke_task_llm(
        &self,
        llm_model: Option<&str>,
        messages: &[(&str, String)],
    ) -> Result<(Completion, i64), LlmFailure> {
        let policy = load_policy();
        let used = load_process(&self.state, self.process_id)
            .await
            .ok()
            .flatten()
            .map(|p| p.tool_invocations_used)
            .unwrap_or(0);

        if remaining_tool_budget(&policy, used) > 0 {
            let message = format!(
                "Tool-calling is enabled (AGENT_PLATFORM_TOOLS_ENABLED with a non-empty \
                 AGENT_PLATFORM_TOOLS_ALLOWLIST and AGENT_PLATFORM_TOOL_BUDGET_PER_RUN={}), but \
                 agent-platformd does not implement the tool path — it lives in Python's \
                 tool_handlers.py. Refusing to run this task without the tools it was configured \
                 to use. Unset AGENT_PLATFORM_TOOLS_ENABLED, or run the Python server directly.",
                policy.budget_per_run
            );
            logd!("{message}");
            return Err(LlmFailure::Unexpected(message));
        }

        let completion = call_llm(&self.state, messages, llm_model, false, 0.7, None).await?;
        Ok((completion, 0))
    }

    /// `services/task_result_service.apply_task_success`. Returns whether the
    /// caller should attempt a sub-DAG expansion.
    async fn apply_task_success(
        &self,
        task: &TaskRow,
        completion: &Completion,
        tool_calls: i64,
    ) -> Result<bool, sqlx::Error> {
        let Some(process) = load_process(&self.state, self.process_id).await? else {
            return Err(sqlx::Error::RowNotFound);
        };

        let (status, completed_at, needs_expand) = if task.requires_review != 0 {
            ("awaiting_review", None, false)
        } else {
            ("completed", Some(sql_now()), true)
        };

        let mut tx = self.state.any.begin().await?;
        sqlx::query(&crate::db::sql(
            "UPDATE tasknode SET output = ?, tokens_used = ?, status = ?, completed_at = ? \
             WHERE id = ?", self.state.backend)
        )
        .bind(&completion.content)
        .bind(completion.tokens)
        .bind(status)
        .bind(&completed_at)
        .bind(task.id)
        .execute(&mut *tx)
        .await?;

        // The review branch also moves the process; the completed branch leaves
        // its status alone and lets the wave loop decide.
        let update = if task.requires_review != 0 {
            "UPDATE process SET total_tokens = total_tokens + ?, total_cost = total_cost + ?, \
             tool_invocations_used = tool_invocations_used + ?, status = 'task_review_required' \
             WHERE id = ?"
        } else {
            "UPDATE process SET total_tokens = total_tokens + ?, total_cost = total_cost + ?, \
             tool_invocations_used = tool_invocations_used + ? WHERE id = ?"
        };
        sqlx::query(&crate::db::sql(update, self.state.backend))
            .bind(completion.tokens)
            .bind(completion.cost)
            .bind(tool_calls)
            .bind(self.process_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        record_api_token_usage(
            &self.state,
            process.token_id,
            completion.tokens,
            completion.cost,
            false,
        )
        .await;
        sync_review_assignments(&self.state, self.process_id).await?;

        self.log(
            "status_change",
            &if task.requires_review != 0 {
                format!("Task {} awaiting review", task.client_uuid)
            } else {
                format!("Task {} completed", task.client_uuid)
            },
            Some(task.id),
        )
        .await;
        self.log("trace", &completion.content, Some(task.id)).await;

        Ok(needs_expand)
    }

    /// `services/task_result_service.apply_task_failure` plus its event.
    async fn record_task_failure(&self, task: &TaskRow, failure: &LlmFailure) {
        let (reason, event) = match failure {
            LlmFailure::Llm(_) => (
                truncate_reason_default(&format!("Task {} failed: {failure}", task.client_uuid)),
                format!("Task {} failed: {failure}", task.client_uuid),
            ),
            LlmFailure::Unexpected(_) => {
                logd!("task {} failed (unexpected): {failure}", task.id);
                (
                    truncate_reason_default(&format!(
                        "Task {} failed: unexpected error",
                        task.client_uuid
                    )),
                    format!("Task {} failed: unexpected error", task.client_uuid),
                )
            }
        };

        let token_id = load_process(&self.state, self.process_id)
            .await
            .ok()
            .flatten()
            .and_then(|p| p.token_id);

        if let Err(e) = self.write_task_failure(task.id, failure, &reason).await {
            logd!("task {} failure could not be stored: {e}", task.id);
        }
        record_api_token_usage(&self.state, token_id, 0, 0.0, true).await;
        self.log("error", &event, Some(task.id)).await;
    }

    async fn write_task_failure(
        &self,
        task_id: i64,
        failure: &LlmFailure,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.state.any.begin().await?;
        sqlx::query(&crate::db::sql("UPDATE tasknode SET status = 'failed', failure_debug_json = ? WHERE id = ?", self.state.backend))
            .bind(task_failure_debug_json(failure))
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&crate::db::sql("UPDATE process SET status = 'failed', failure_reason = ? WHERE id = ?", self.state.backend))
            .bind(reason)
            .bind(self.process_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    // -- sub-DAG expansion -------------------------------------------------

    async fn maybe_expand_subdag_after_success(&self, task_id: i64) {
        let max_expansions = subdecomp_max_expansions();
        let max_new_tasks = subdecomp_max_new_tasks();
        if max_expansions <= 0 || max_new_tasks <= 0 {
            return;
        }

        // Held across the LLM call, exactly as Python holds its `asyncio.Lock`:
        // the counters and the read-modify-write of `dag_json` are one unit.
        let mut subdecomp = self.subdecomp.lock().await;

        let task: Option<TaskRow> =
            match sqlx::query_as(&crate::db::sql(&format!("SELECT {TASK_COLUMNS} FROM tasknode WHERE id = ?"), self.state.backend))
                .bind(task_id)
                .fetch_optional(&self.state.any)
                .await
            {
                Ok(row) => row,
                Err(e) => {
                    logd!("sub-DAG expansion could not read task: {e}");
                    return;
                }
            };
        let Some(task) = task else { return };
        let Ok(Some(process)) = load_process(&self.state, self.process_id).await else { return };
        let Some(dag_json) = process.dag_json.clone().filter(|d| !d.is_empty()) else { return };

        let planner = match serde_json::from_str::<Value>(&dag_json)
            .map_err(|e| e.to_string())
            .and_then(|raw| validate_planner_dag(&raw))
        {
            Ok(planner) => planner,
            Err(e) => {
                logd!("sub-DAG expansion skipped (invalid dag_json): {e}");
                return;
            }
        };

        let parent_depth = subdecomp.uuid_depth.get(&task.client_uuid).copied().unwrap_or(0);
        let allowed = expansion_allowed(&ExpansionGate {
            process_status: &process.status,
            expansions_used: subdecomp.expansions_used,
            new_tasks_added: subdecomp.new_tasks_added,
            max_expansions,
            max_new_tasks,
            spec: planner.spec(&task.client_uuid),
            parent_depth,
            max_depth: subdecomp_max_depth(),
        });
        if !allowed {
            return;
        }

        let existing_uuids: HashSet<String> = planner.uuids().into_iter().collect();
        let parent_output: Option<String> =
            sqlx::query_scalar(&crate::db::sql("SELECT output FROM tasknode WHERE id = ?", self.state.backend))
                .bind(task_id)
                .fetch_optional(&self.state.any)
                .await
                .ok()
                .flatten()
                .flatten();
        let remaining_slots = max_new_tasks - subdecomp.new_tasks_added;

        let expansion = match generate_subdag_expansion(
            &self.state,
            &process.goal,
            &task.client_uuid,
            &task.role,
            &parent_output.unwrap_or_default(),
            &existing_uuids,
        )
        .await
        {
            Ok(expansion) => expansion,
            Err(e) => {
                logd!("sub-DAG expansion LLM failed: {e}");
                self.log("error", &format!("Sub-DAG expansion skipped: {e}"), None).await;
                return;
            }
        };

        let Some(new_specs) = cap_expansion(expansion.subagents, remaining_slots) else { return };

        let created = match self
            .merge_and_persist_subdag_expansion(
                &planner,
                &new_specs,
                expansion.tokens,
                expansion.cost,
                &task.client_uuid,
                process.token_id,
            )
            .await
        {
            Ok(created) => created,
            Err(e) => {
                logd!("sub-DAG merge validation failed: {e}");
                self.log("error", &format!("Sub-DAG expansion merge failed: {e}"), None).await;
                return;
            }
        };

        subdecomp.expansions_used += 1;
        subdecomp.new_tasks_added += created;
        let child_depth = parent_depth + 1;
        for spec in &new_specs {
            if !spec.client_uuid.is_empty() {
                subdecomp.uuid_depth.insert(spec.client_uuid.clone(), child_depth);
            }
        }
        drop(subdecomp);

        self.log(
            "status_change",
            &format!("Sub-DAG expansion added {created} task(s) after {}", task.client_uuid),
            None,
        )
        .await;
    }

    /// `services/subdag_service.merge_and_persist_subdag_expansion`.
    async fn merge_and_persist_subdag_expansion(
        &self,
        planner: &PlannerDag,
        new_specs: &[SubagentSpec],
        add_tokens: i64,
        add_cost: f64,
        parent_uuid: &str,
        token_id: Option<i64>,
    ) -> Result<i64, String> {
        let merged = merge_planner_with_new_subagents(planner, new_specs)?;
        // `json.dumps` with the default `ensure_ascii=True`.
        let merged_json = planner_dag_to_json(&merged, true);

        let status: Option<String> =
            sqlx::query_scalar(&crate::db::sql("SELECT status FROM process WHERE id = ?", self.state.backend))
                .bind(self.process_id)
                .fetch_optional(&self.state.any)
                .await
                .map_err(|e| e.to_string())?;
        if status.as_deref() != Some("running") {
            return Ok(0);
        }

        let existing: Vec<String> =
            sqlx::query_scalar(&crate::db::sql("SELECT client_uuid FROM tasknode WHERE process_id = ?", self.state.backend))
                .bind(self.process_id)
                .fetch_all(&self.state.any)
                .await
                .map_err(|e| e.to_string())?;
        let existing: HashSet<String> = existing.into_iter().collect();

        let mut tx = self.state.any.begin().await.map_err(|e| e.to_string())?;
        sqlx::query(&crate::db::sql(
            "UPDATE process SET dag_json = ?, total_tokens = total_tokens + ?, \
             total_cost = total_cost + ? WHERE id = ?", self.state.backend)
        )
        .bind(&merged_json)
        .bind(add_tokens)
        .bind(add_cost)
        .bind(self.process_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut created = 0i64;
        for spec in new_specs {
            if existing.contains(&spec.client_uuid) {
                continue;
            }
            insert_task_node(&mut tx, self.state.backend, self.process_id, spec, Some(parent_uuid))
                .await
                .map_err(|e| e.to_string())?;
            created += 1;
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        record_api_token_usage(&self.state, token_id, add_tokens, add_cost, false).await;
        Ok(created)
    }
}

struct TaskInputs {
    task: TaskRow,
    deps_texts: Vec<String>,
    system_message: String,
    user_message: String,
}

/// `Duration::from_secs_f64` panics on a value the `f64` cannot represent as a
/// duration; every one of these comes out of an env var a user typed.
fn duration_from_secs_f64(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds.clamp(0.001, 1.0e9))
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Run `fut` detached, and if it panics, fail the process rather than letting
/// the row sit at `planning`/`running` forever.
///
/// `tokio::spawn` already keeps a panic from reaching the runtime; what this
/// adds is the database write, which is the part a user would otherwise notice.
fn spawn_guarded<F>(state: Arc<AppState>, process_id: i64, what: &'static str, fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        if std::panic::AssertUnwindSafe(fut).catch_unwind().await.is_err() {
            let reason = format!("{what} crashed unexpectedly (agent-platformd bug)");
            logd!("process {process_id}: {reason}");
            let _ = fail_process(&state, process_id, &reason).await;
            append_event(&state, process_id, None, "error", &reason).await;
        }
    });
}

/// `DAGExecutor(process_id, auto_approve).plan(goal, team_context)` on
/// `BackgroundTasks`. Routes 2, 8 and 9.
pub fn spawn_plan(
    state: Arc<AppState>,
    process_id: i64,
    goal: String,
    team_context: Option<String>,
    auto_approve: bool,
) {
    let executor = Executor::new(Arc::clone(&state), process_id, auto_approve);
    spawn_guarded(state, process_id, "Planning", async move {
        executor.plan(goal, team_context).await
    });
}

/// `DAGExecutor(process_id).execute_dag()` on `BackgroundTasks`. Routes 5, 6, 8,
/// 9 and 10.
pub fn spawn_execute_dag(state: Arc<AppState>, process_id: i64) {
    let executor = Executor::new(Arc::clone(&state), process_id, false);
    spawn_guarded(state, process_id, "DAG execution", async move {
        executor.execute_dag().await
    });
}

/// `DAGExecutor(process_id).expand_after_review_approval_and_continue(task_id)`.
/// Route 6, on the approve branch of a task review.
pub fn spawn_expand_after_review(state: Arc<AppState>, process_id: i64, task_id: i64) {
    let executor = Executor::new(Arc::clone(&state), process_id, false);
    spawn_guarded(state, process_id, "DAG execution", async move {
        executor.maybe_expand_subdag_after_success(task_id).await;
        executor.execute_dag().await;
    });
}

// ---------------------------------------------------------------------------
// Startup recovery (`app/services/startup_recovery.py`)
// ---------------------------------------------------------------------------

const RECOVERABLE_STATUSES: [&str; 4] = ["pending", "planning", "approved", "running"];

/// What `POST /processes/{id}/sync` would do to an interrupted row, decided at
/// boot. Human gates (`approval_required`, `task_review_required`) and terminal
/// statuses are not in the query at all.
#[derive(Debug, PartialEq)]
enum Recovery {
    Replan,
    RequeueApproved,
    /// `approved` with no DAG: there is nothing to execute, so it is left alone.
    SkipApprovedWithoutDag,
    AlignReview,
    RequeueRunning,
}

fn recovery_action(status: &str, dag_json: Option<&str>, awaiting_review: usize) -> Option<Recovery> {
    match status {
        "pending" | "planning" => Some(Recovery::Replan),
        "approved" => Some(if dag_json.map(str::trim).is_some_and(|d| !d.is_empty()) {
            Recovery::RequeueApproved
        } else {
            Recovery::SkipApprovedWithoutDag
        }),
        "running" => {
            Some(if awaiting_review > 0 { Recovery::AlignReview } else { Recovery::RequeueRunning })
        }
        _ => None,
    }
}

#[derive(Debug, Default, PartialEq)]
struct RecoveryCounts {
    replanned: usize,
    requeued: usize,
    aligned_review: usize,
    skipped: usize,
}

/// Requeue work a restart interrupted.
///
/// `AGENT_PLATFORM_RESUME_ON_STARTUP=0` switches it off. That flag, and the
/// matching one on the workflow scheduler, existed because a second server —
/// the Python child — was reading the same tables and would plan every
/// interrupted process twice. There is no second server now, so the flag is
/// what it reads like: "start without replaying anything".
pub fn spawn_startup_recovery(state: Arc<AppState>) {
    if !resume_on_startup_enabled() {
        logd!(
            "startup recovery disabled (AGENT_PLATFORM_RESUME_ON_STARTUP)"
        );
        return;
    }
    tokio::spawn(async move {
        let run = std::panic::AssertUnwindSafe(recover_interrupted_processes(state.clone()))
            .catch_unwind()
            .await;
        match run {
            Ok(Ok(counts)) => {
                if counts != RecoveryCounts::default() {
                    logd!("startup recovery: {counts:?}");
                }
            }
            Ok(Err(e)) => logd!("startup recovery failed: {e}"),
            Err(_) => logd!("startup recovery panicked"),
        }
    });
}

#[derive(FromRow)]
struct RecoveryRow {
    id: i64,
    goal: String,
    status: String,
    dag_json: Option<String>,
    team_snapshot_json: Option<String>,
}

async fn recover_interrupted_processes(
    state: Arc<AppState>,
) -> Result<RecoveryCounts, sqlx::Error> {
    let placeholders = vec!["?"; RECOVERABLE_STATUSES.len()].join(", ");
    let sql = format!(
        "SELECT id, goal, status, dag_json, team_snapshot_json FROM process \
         WHERE status IN ({placeholders})"
    );
    // Bound to a local: the query borrows the rewritten string while the binds
    // are added one at a time.
    let sql = crate::db::sql(&sql, state.backend).into_owned();
    let mut query = sqlx::query_as::<_, RecoveryRow>(&sql);
    for status in RECOVERABLE_STATUSES {
        query = query.bind(status);
    }
    let rows = query.fetch_all(&state.any).await?;

    let mut counts = RecoveryCounts::default();
    let mut plans: Vec<(i64, String, Option<String>)> = Vec::new();
    let mut executions: Vec<i64> = Vec::new();

    for row in rows {
        let awaiting_review: i64 = sqlx::query_scalar(&crate::db::sql(
            "SELECT COUNT(*) FROM tasknode WHERE process_id = ? AND status = 'awaiting_review'", state.backend)
        )
        .bind(row.id)
        .fetch_one(&state.any)
        .await?;

        let action =
            recovery_action(&row.status, row.dag_json.as_deref(), awaiting_review as usize);
        match action {
            None => continue,
            Some(Recovery::Replan) => {
                plans.push((
                    row.id,
                    row.goal.clone(),
                    team_context_from_snapshot_json(row.team_snapshot_json.as_deref()),
                ));
                append_event(
                    &state,
                    row.id,
                    None,
                    "status_change",
                    "Startup recovery: re-scheduled planning after server restart",
                )
                .await;
                counts.replanned += 1;
            }
            Some(Recovery::SkipApprovedWithoutDag) => {
                logd!(
                    "startup recovery: process {} approved without DAG JSON; \
                     skipping",
                    row.id
                );
                counts.skipped += 1;
            }
            Some(Recovery::RequeueApproved) => {
                executions.push(row.id);
                append_event(
                    &state,
                    row.id,
                    None,
                    "status_change",
                    "Startup recovery: re-scheduled DAG execution after server restart",
                )
                .await;
                counts.requeued += 1;
            }
            Some(Recovery::AlignReview) => {
                sqlx::query(&crate::db::sql(
                    "UPDATE process SET status = 'task_review_required', failure_reason = NULL \
                     WHERE id = ?", state.backend)
                )
                .bind(row.id)
                .execute(&state.any)
                .await?;
                append_event(
                    &state,
                    row.id,
                    None,
                    "status_change",
                    "Startup recovery: aligned status to task_review_required (review gate open)",
                )
                .await;
                counts.aligned_review += 1;
            }
            Some(Recovery::RequeueRunning) => {
                let reset = sqlx::query(&crate::db::sql(
                    "UPDATE tasknode SET status = 'pending', output = NULL, draft_output = NULL, \
                     review_feedback = NULL, reviewer_client_uuid = NULL, \
                     failure_debug_json = NULL, started_at = NULL, completed_at = NULL, \
                     tokens_used = 0 WHERE process_id = ? AND status = 'running'", state.backend)
                )
                .bind(row.id)
                .execute(&state.any)
                .await?
                .rows_affected();
                sqlx::query(&crate::db::sql("UPDATE process SET failure_reason = NULL WHERE id = ?", state.backend))
                    .bind(row.id)
                    .execute(&state.any)
                    .await?;
                executions.push(row.id);
                append_event(
                    &state,
                    row.id,
                    None,
                    "status_change",
                    &format!(
                        "Startup recovery: reset {reset} stuck running task(s) to pending; \
                         re-scheduled DAG execution after server restart"
                    ),
                )
                .await;
                counts.requeued += 1;
            }
        }
    }

    for (process_id, goal, team_context) in plans {
        spawn_plan(Arc::clone(&state), process_id, goal, team_context, false);
    }
    for process_id in executions {
        spawn_execute_dag(Arc::clone(&state), process_id);
    }
    Ok(counts)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(uuid: &str, role: &str, deps: &[&str]) -> SubagentSpec {
        SubagentSpec {
            client_uuid: uuid.into(),
            role: role.into(),
            system_prompt: "s".into(),
            instructions: "i".into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            llm_model: None,
            subdecompose: false,
            requires_review: false,
        }
    }

    fn pending(id: i64, deps: &[&str]) -> PendingTask {
        PendingTask {
            id,
            client_uuid: format!("t{id}"),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn statuses(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(u, s)| (u.to_string(), s.to_string())).collect()
    }

    fn completed(uuids: &[&str]) -> HashSet<String> {
        uuids.iter().map(|u| u.to_string()).collect()
    }

    fn review_task(id: i64, uuid: &str, status: &str, deps: &[&str]) -> ReviewTask {
        ReviewTask {
            id,
            client_uuid: uuid.into(),
            status: status.into(),
            dependencies_json: json!(deps).to_string(),
            reviewer_client_uuid: None,
        }
    }

    // -- wave readiness and ordering ---------------------------------------

    /// `test_dag_executor.py::test_execute_dag_batches_ready_tasks_when_max_concurrent_is_one`
    /// — the FIFO-by-id rule and the cap, which nothing else covers.
    #[test]
    fn ready_tasks_run_fifo_by_id_and_respect_the_concurrency_cap() {
        // Insertion order is not id order here; the wave must not care.
        let tasks = vec![pending(9, &[]), pending(3, &[]), pending(7, &[])];
        let none = completed(&[]);
        assert_eq!(select_ready_task_ids(&tasks, &none, None), vec![3, 7, 9]);
        assert_eq!(select_ready_task_ids(&tasks, &none, Some(1)), vec![3]);
        assert_eq!(select_ready_task_ids(&tasks, &none, Some(2)), vec![3, 7]);
        // A cap larger than the wave is not an error.
        assert_eq!(select_ready_task_ids(&tasks, &none, Some(99)), vec![3, 7, 9]);
    }

    #[test]
    fn a_task_waits_until_every_dependency_is_completed() {
        let tasks = vec![pending(1, &["a"]), pending(2, &["a", "b"]), pending(3, &[])];
        assert_eq!(select_ready_task_ids(&tasks, &completed(&[]), None), vec![3]);
        assert_eq!(select_ready_task_ids(&tasks, &completed(&["a"]), None), vec![1, 3]);
        assert_eq!(select_ready_task_ids(&tasks, &completed(&["a", "b"]), None), vec![1, 2, 3]);
        // `awaiting_review` and `failed` are not `completed`, so nothing unblocks.
        assert_eq!(select_ready_task_ids(&tasks, &completed(&["b"]), None), vec![3]);
    }

    // -- the four wave outcomes --------------------------------------------

    #[test]
    fn an_empty_pending_set_completes_the_process() {
        let snapshot = DagSnapshot::default();
        assert_eq!(plan_wave(&snapshot, None), Wave::Complete);
    }

    /// `test_requires_review_sets_awaiting_review_and_pauses_run` and
    /// `test_execute_dag_no_deadlock_when_dependent_pending_and_upstream_awaiting_review`:
    /// the review gate must beat the deadlock branch, both when nothing is
    /// pending and when a dependent is stuck behind the gate.
    #[test]
    fn an_open_review_gate_pauses_instead_of_completing_or_deadlocking() {
        let paused = DagSnapshot { awaiting_review_exists: true, ..Default::default() };
        assert_eq!(plan_wave(&paused, None), Wave::PauseForReview);

        // B depends on A; A sits in awaiting_review. Not a deadlock.
        let blocked = DagSnapshot {
            pending_tasks: vec![pending(2, &["a"])],
            awaiting_review_exists: true,
            ..Default::default()
        };
        assert_eq!(plan_wave(&blocked, None), Wave::PauseForReview);
    }

    /// The deadlock branch — pending work, nothing runnable, no human holding
    /// anything. Nothing else in this port covers it.
    #[test]
    fn unsatisfiable_dependencies_are_a_deadlock_not_a_hang() {
        let cycle = DagSnapshot {
            pending_tasks: vec![pending(1, &["t2"]), pending(2, &["t1"])],
            status_by_uuid: statuses(&[("t1", "pending"), ("t2", "pending")]),
            ..Default::default()
        };
        let Wave::Deadlock(reason) = plan_wave(&cycle, None) else { panic!("expected deadlock") };
        assert!(reason.contains("cyclic dependencies among task(s) t1, t2"), "{reason}");

        // A dependency on a task that failed is the same shape, but a different
        // fix: the user retries that task, not the graph.
        let orphaned = DagSnapshot {
            pending_tasks: vec![pending(1, &["a"])],
            status_by_uuid: statuses(&[("a", "failed"), ("t1", "pending")]),
            ..Default::default()
        };
        let Wave::Deadlock(reason) = plan_wave(&orphaned, None) else { panic!("expected deadlock") };
        assert!(reason.contains("blocked by failed task(s) a"), "{reason}");

        // A dependency on a uuid that is not a task at all.
        let missing = DagSnapshot {
            pending_tasks: vec![pending(1, &["gone"])],
            status_by_uuid: statuses(&[("t1", "pending")]),
            ..Default::default()
        };
        let Wave::Deadlock(reason) = plan_wave(&missing, None) else { panic!("expected deadlock") };
        assert!(reason.contains("do not exist: gone"), "{reason}");
    }

    #[test]
    fn a_runnable_wave_is_the_capped_ready_list() {
        let snapshot = DagSnapshot {
            pending_tasks: vec![pending(5, &[]), pending(2, &[]), pending(8, &["a"])],
            completed_uuids: completed(&["a"]),
            ..Default::default()
        };
        assert_eq!(plan_wave(&snapshot, None), Wave::Run(vec![2, 5, 8]));
        assert_eq!(plan_wave(&snapshot, Some(2)), Wave::Run(vec![2, 5]));
    }

    // -- the run budget ----------------------------------------------------

    #[test]
    fn the_run_budget_only_counts_when_it_parses_and_is_positive() {
        // `_run_max_seconds` and friends: blank, junk and non-positive are all
        // "unlimited", which is what stops a typo from failing every process.
        let key = "AGENT_PLATFORM_RUN_MAX_SECONDS";
        for (raw, expected) in
            [("", None), ("   ", None), ("junk", None), ("0", None), ("-5", None), ("1.5", Some(1.5))]
        {
            std::env::set_var(key, raw);
            assert_eq!(env_positive_f64(key), expected, "{raw:?}");
        }
        std::env::remove_var(key);
        assert_eq!(env_positive_f64(key), None);

        // A budget that has passed is what fails the run; the comparison is the
        // whole of the branch.
        let deadline = Instant::now() - Duration::from_millis(1);
        assert!(Some(deadline).is_some_and(|d| Instant::now() > d));
        let future = Instant::now() + Duration::from_secs(60);
        assert!(!Some(future).is_some_and(|d| Instant::now() > d));

        // A wild value must not panic `Duration::from_secs_f64`.
        assert!(duration_from_secs_f64(f64::INFINITY) <= Duration::from_secs_f64(1.0e9));
        assert!(duration_from_secs_f64(0.0) > Duration::ZERO);
    }

    #[test]
    fn the_concurrency_cap_and_expansion_caps_read_their_env_pythons_way() {
        let key = "AGENT_PLATFORM_SUBDECOMP_MAX_EXPANSIONS";
        std::env::remove_var(key);
        // Unset uses the default, which is > 0 so expansion works out of the box.
        assert_eq!(env_count_or_zero(key, 48), 48);
        std::env::set_var(key, "0");
        assert_eq!(env_count_or_zero(key, 48), 0);
        std::env::set_var(key, "-3");
        assert_eq!(env_count_or_zero(key, 48), 0);
        // Junk disables it rather than falling back to the default.
        std::env::set_var(key, "lots");
        assert_eq!(env_count_or_zero(key, 48), 0);
        std::env::remove_var(key);
    }

    // -- the review gate ---------------------------------------------------

    /// `test_execute_dag_no_deadlock_when_dependent_pending_and_upstream_awaiting_review`
    /// asserts `ta.reviewer_client_uuid == "b"`: the downstream peer, which is
    /// pending with an unsatisfied dependency and therefore idle.
    #[test]
    fn a_downstream_peer_that_cannot_run_yet_is_the_reviewer() {
        let subagents = vec![spec("a", "A", &[]), spec("b", "B", &["a"])];
        let tasks = vec![
            review_task(1, "a", "awaiting_review", &[]),
            review_task(2, "b", "pending", &["a"]),
        ];
        assert_eq!(
            compute_review_assignments(&tasks, &subagents),
            vec![(1, Some("b".to_string()))]
        );
    }

    /// `test_reviewer_assigned_when_parallel_peer_completes`: an independent
    /// peer that already finished can review.
    #[test]
    fn a_completed_parallel_peer_is_a_valid_reviewer() {
        let subagents = vec![spec("a", "Author", &[]), spec("b", "Peer", &[])];
        let tasks = vec![
            review_task(1, "a", "awaiting_review", &[]),
            review_task(2, "b", "completed", &[]),
        ];
        assert_eq!(
            compute_review_assignments(&tasks, &subagents),
            vec![(1, Some("b".to_string()))]
        );
    }

    #[test]
    fn nobody_idle_means_no_reviewer_and_no_write() {
        // The only peer is running, which is neither completed nor idle-pending.
        let subagents = vec![spec("a", "A", &[]), spec("b", "B", &[])];
        let tasks = vec![
            review_task(1, "a", "awaiting_review", &[]),
            review_task(2, "b", "running", &[]),
        ];
        assert!(compute_review_assignments(&tasks, &subagents).is_empty());

        // A pending peer with no dependencies is about to be scheduled, so it
        // is not idle either.
        let tasks = vec![
            review_task(1, "a", "awaiting_review", &[]),
            review_task(2, "b", "pending", &[]),
        ];
        assert!(compute_review_assignments(&tasks, &subagents).is_empty());
    }

    #[test]
    fn a_reviewer_that_went_stale_is_cleared_even_when_nobody_replaces_it() {
        let subagents = vec![spec("a", "A", &[]), spec("b", "B", &[])];
        let mut tasks = vec![
            review_task(1, "a", "awaiting_review", &[]),
            // `b` has since started running, so it can no longer review.
            review_task(2, "b", "running", &[]),
        ];
        tasks[0].reviewer_client_uuid = Some("b".into());
        assert_eq!(compute_review_assignments(&tasks, &subagents), vec![(1, None)]);

        // A reviewer that is still valid is left alone — no write at all.
        let mut tasks = vec![
            review_task(1, "a", "awaiting_review", &[]),
            review_task(2, "b", "completed", &[]),
        ];
        tasks[0].reviewer_client_uuid = Some("b".into());
        assert!(compute_review_assignments(&tasks, &subagents).is_empty());
    }

    #[test]
    fn reviewer_scoring_prefers_downstream_then_upstream_then_the_lowest_uuid() {
        // `z` is downstream (100 + 80), `b` is upstream (40), `m` is unrelated.
        let subagents = vec![
            spec("b", "Editor", &[]),
            spec("a", "Writer", &["b"]),
            spec("m", "Other", &[]),
            spec("z", "Reviewer", &["a"]),
        ];
        let tasks = vec![
            review_task(1, "a", "awaiting_review", &["b"]),
            review_task(2, "b", "completed", &[]),
            review_task(3, "m", "completed", &[]),
            review_task(4, "z", "completed", &[]),
        ];
        assert_eq!(
            compute_review_assignments(&tasks, &subagents),
            vec![(1, Some("z".to_string()))]
        );

        // With the downstream peer gone, the upstream one wins over the stranger.
        let subagents: Vec<_> = subagents.into_iter().filter(|s| s.client_uuid != "z").collect();
        let tasks: Vec<_> = tasks.into_iter().filter(|t| t.client_uuid != "z").collect();
        assert_eq!(
            compute_review_assignments(&tasks, &subagents),
            vec![(1, Some("b".to_string()))]
        );
    }

    #[test]
    fn a_shared_role_word_breaks_a_tie_before_the_uuid_does() {
        assert_eq!(role_word_overlap("Backend Engineer", "backend tester"), 1);
        // Hyphens are word separators, and the compare is case-insensitive.
        assert_eq!(role_word_overlap("TypeScript-Expert", "expert reviewer"), 1);
        assert_eq!(role_word_overlap("Writer", "Editor"), 0);

        let subagents =
            vec![spec("a", "Docs Writer", &[]), spec("x", "Docs Reviewer", &[]), spec("b", "Other", &[])];
        let tasks = vec![
            review_task(1, "a", "awaiting_review", &[]),
            review_task(2, "x", "completed", &[]),
            review_task(3, "b", "completed", &[]),
        ];
        // `b` sorts first alphabetically but shares no role word with `a`.
        assert_eq!(
            compute_review_assignments(&tasks, &subagents),
            vec![(1, Some("x".to_string()))]
        );
    }

    // -- sub-DAG expansion caps --------------------------------------------

    fn gate(spec: Option<&SubagentSpec>) -> ExpansionGate<'_> {
        ExpansionGate {
            process_status: "running",
            expansions_used: 0,
            new_tasks_added: 0,
            max_expansions: 48,
            max_new_tasks: 48,
            spec,
            parent_depth: 0,
            max_depth: None,
        }
    }

    /// `test_subdag_expansion_adds_nodes_and_merges_dag` only exercises the
    /// happy path; every refusal below has no other coverage.
    #[test]
    fn expansion_needs_a_running_process_a_subdecompose_node_and_free_budget() {
        let mut node = spec("a", "A", &[]);
        node.subdecompose = true;

        assert!(expansion_allowed(&gate(Some(&node))));

        // A node the planner did not mark, or one behind a review gate.
        assert!(!expansion_allowed(&gate(Some(&spec("a", "A", &[])))));
        let mut gated = node.clone();
        gated.requires_review = true;
        assert!(!expansion_allowed(&gate(Some(&gated))));
        // A task whose uuid is not in the DAG at all (a stale row).
        assert!(!expansion_allowed(&gate(None)));

        // Only a running process expands: a cancelled one must not grow.
        for status in ["cancelled", "failed", "task_review_required", "completed"] {
            let mut g = gate(Some(&node));
            g.process_status = status;
            assert!(!expansion_allowed(&g), "{status}");
        }

        // Both caps, at the boundary.
        let mut g = gate(Some(&node));
        g.expansions_used = 48;
        assert!(!expansion_allowed(&g));
        let mut g = gate(Some(&node));
        g.new_tasks_added = 48;
        assert!(!expansion_allowed(&g));
        let mut g = gate(Some(&node));
        g.expansions_used = 47;
        assert!(expansion_allowed(&g));

        // Either cap at zero disables expansion outright.
        let mut g = gate(Some(&node));
        g.max_expansions = 0;
        assert!(!expansion_allowed(&g));
        let mut g = gate(Some(&node));
        g.max_new_tasks = 0;
        assert!(!expansion_allowed(&g));
    }

    #[test]
    fn the_depth_cap_stops_a_child_from_expanding_forever() {
        let mut node = spec("a", "A", &[]);
        node.subdecompose = true;

        let mut g = gate(Some(&node));
        g.max_depth = Some(1);
        g.parent_depth = 0;
        assert!(expansion_allowed(&g), "a planner task may spawn depth 1");
        g.parent_depth = 1;
        assert!(!expansion_allowed(&g), "its children may not spawn depth 2");

        // Unset is unlimited.
        g.max_depth = None;
        g.parent_depth = 99;
        assert!(expansion_allowed(&g));
    }

    #[test]
    fn an_expansion_is_trimmed_to_the_slots_that_are_left() {
        let specs = vec![spec("c1", "C", &["a"]), spec("c2", "C", &["a"]), spec("c3", "C", &["a"])];
        assert_eq!(cap_expansion(specs.clone(), 2).unwrap().len(), 2);
        assert_eq!(cap_expansion(specs.clone(), 10).unwrap().len(), 3);
        // Slots exhausted while the model was thinking: the whole batch is dropped.
        assert!(cap_expansion(specs.clone(), 0).is_none());
        assert!(cap_expansion(specs, -1).is_none());
    }

    /// The two rules the expansion adds on top of the schema, both of which
    /// come back as a retry rather than a bad DAG.
    #[test]
    fn an_expansion_must_be_new_uuids_that_all_depend_on_the_parent() {
        let existing: HashSet<String> = ["a".to_string()].into_iter().collect();
        let ok = json!({"subagents": [{
            "client_uuid": "c1", "role": "C", "system_prompt": "s",
            "instructions": "i", "dependencies": ["a"],
        }]});
        assert_eq!(parse_expansion(&ok, "a", &existing).unwrap().len(), 1);

        let reused = json!({"subagents": [{
            "client_uuid": "a", "role": "C", "system_prompt": "s",
            "instructions": "i", "dependencies": ["a"],
        }]});
        assert_eq!(
            parse_expansion(&reused, "a", &existing).unwrap_err(),
            "Sub-decomposition reused existing client_uuid: 'a'"
        );

        let detached = json!({"subagents": [{
            "client_uuid": "c1", "role": "C", "system_prompt": "s",
            "instructions": "i", "dependencies": [],
        }]});
        assert_eq!(
            parse_expansion(&detached, "a", &existing).unwrap_err(),
            "Sub-decomposition subagent 'c1' must depend on parent 'a'"
        );

        assert!(parse_expansion(&json!({"subagents": []}), "a", &existing).is_err());
        assert!(parse_expansion(&json!({}), "a", &existing).is_err());
    }

    // -- the tool branch ---------------------------------------------------

    #[test]
    fn the_tool_branch_only_fires_with_all_three_switches_on() {
        let off = ToolPolicy { enabled: false, allowlist: vec!["echo".into()], budget_per_run: 10 };
        assert_eq!(remaining_tool_budget(&off, 0), 0);
        // Enabled with an empty allowlist is Python's `is_allowed == false`: the
        // plain-completion branch, which this port implements, so it must not
        // be refused.
        let no_list = ToolPolicy { enabled: true, allowlist: vec![], budget_per_run: 10 };
        assert_eq!(remaining_tool_budget(&no_list, 0), 0);
        let no_budget =
            ToolPolicy { enabled: true, allowlist: vec!["echo".into()], budget_per_run: 0 };
        assert_eq!(remaining_tool_budget(&no_budget, 0), 0);

        let on = ToolPolicy { enabled: true, allowlist: vec!["echo".into()], budget_per_run: 10 };
        assert_eq!(remaining_tool_budget(&on, 0), 10);
        assert_eq!(remaining_tool_budget(&on, 4), 6);
        // A run that already spent its budget falls back to plain completion.
        assert_eq!(remaining_tool_budget(&on, 10), 0);
        assert_eq!(remaining_tool_budget(&on, 99), 0);
    }

    // -- planner retries ---------------------------------------------------

    /// `test_planner_retries.py`: the fallback alias is used on the **last**
    /// attempt only, and only when it differs from `PLANNER_MODEL`.
    #[test]
    fn the_fallback_model_is_reserved_for_the_last_attempt() {
        std::env::set_var("PLANNER_MODEL", "local");
        std::env::remove_var("SUBAGENT_MODEL");

        assert_eq!(model_for_plan_attempt(0, 3, Some("strong")).as_deref(), Some("local"));
        assert_eq!(model_for_plan_attempt(1, 3, Some("strong")).as_deref(), Some("local"));
        assert_eq!(model_for_plan_attempt(2, 3, Some("strong")).as_deref(), Some("strong"));
        // No fallback configured: every attempt uses the primary.
        assert_eq!(model_for_plan_attempt(2, 3, None).as_deref(), Some("local"));

        // A fallback equal to the primary is not a fallback.
        std::env::set_var("PLANNER_FALLBACK_MODEL", "local");
        assert_eq!(planner_fallback_model(), None);
        std::env::set_var("PLANNER_FALLBACK_MODEL", "strong");
        assert_eq!(planner_fallback_model().as_deref(), Some("strong"));

        std::env::remove_var("PLANNER_MODEL");
        std::env::remove_var("PLANNER_FALLBACK_MODEL");
    }

    #[test]
    fn attempt_counts_never_drop_below_one() {
        let key = "AGENT_PLATFORM_PLAN_MAX_ATTEMPTS";
        std::env::remove_var(key);
        assert_eq!(plan_max_attempts(), 3);
        std::env::set_var(key, "0");
        assert_eq!(plan_max_attempts(), 1);
        std::env::set_var(key, "junk");
        assert_eq!(plan_max_attempts(), 3);
        std::env::set_var(key, "5");
        assert_eq!(plan_max_attempts(), 5);
        std::env::remove_var(key);
    }

    // -- prompts and failure text ------------------------------------------

    #[test]
    fn the_planner_user_message_only_adds_a_roster_when_there_is_one() {
        assert_eq!(planner_user_message("ship it", None), "Goal: ship it");
        assert_eq!(planner_user_message("ship it", Some("   ")), "Goal: ship it");
        assert_eq!(
            planner_user_message("ship it", Some("  Team template: X  ")),
            "Goal: ship it\n\nTeam template: X"
        );
    }

    #[test]
    fn the_subdag_prompt_names_the_parent_everywhere_python_does() {
        let prompt = subdag_system_prompt("agent_1");
        assert_eq!(prompt.matches("agent_1").count(), 4);
        assert!(prompt.starts_with("You extend an existing execution DAG."));
        // The braces of the JSON example survive the format!.
        assert!(prompt.contains("\"subagents\": ["));
        assert!(prompt.ends_with("(acyclic).\n"));
    }

    #[test]
    fn a_revision_run_replays_the_draft_and_the_feedback() {
        // `test_revision_prompt_includes_feedback`.
        let preamble = revision_user_preamble(Some("first draft"), Some("be shorter"));
        assert!(preamble.contains("Previous attempt:\nfirst draft"));
        assert!(preamble.contains("Reviewer feedback:\nbe shorter"));
        assert!(preamble.ends_with("Revise your output to address the feedback above.\n\n"));

        // Either half alone still produces a preamble; neither produces none.
        assert!(revision_user_preamble(Some("d"), None).starts_with("Previous attempt:"));
        assert!(revision_user_preamble(None, Some("f")).starts_with("Reviewer feedback:"));
        assert_eq!(revision_user_preamble(None, None), "");
        // Empty strings are falsy in Python.
        assert_eq!(revision_user_preamble(Some(""), Some("")), "");
    }

    #[test]
    fn a_failure_reason_is_trimmed_by_characters_not_bytes() {
        assert_eq!(truncate_reason("  short  ", 2048), "short");
        assert_eq!(truncate_reason("abcdefgh", 5), "ab...");
        // Multi-byte text must not be cut mid-codepoint.
        let long = "é".repeat(3000);
        let cut = truncate_reason(&long, 2048);
        assert_eq!(cut.chars().count(), 2048);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn failure_debug_json_keeps_its_key_order() {
        let json = task_failure_debug_json(&LlmFailure::Llm("boom".into()));
        assert_eq!(
            json,
            r#"{"source": "llm", "exception_type": "LLMRequestError", "message": "boom"}"#
        );
        assert!(task_failure_debug_json(&LlmFailure::Unexpected("x".into()))
            .starts_with(r#"{"source": "unexpected""#));
    }

    #[test]
    fn cost_is_read_from_whichever_field_the_backend_used() {
        assert_eq!(usage_cost_from_completion_response(&json!({"usage": {"cost": 0.25}})), 0.25);
        assert_eq!(
            usage_cost_from_completion_response(&json!({"usage": {"total_cost": "0.5"}})),
            0.5
        );
        assert_eq!(
            usage_cost_from_completion_response(
                &json!({"usage": {"response_cost": {"total_cost": 1.5}}})
            ),
            1.5
        );
        assert_eq!(usage_cost_from_completion_response(&json!({"response_cost": 2.0})), 2.0);
        assert_eq!(
            usage_cost_from_completion_response(&json!({"_hidden_params": {"response_cost": 3.0}})),
            3.0
        );
        // Plain Ollama reports none, which is 0.0 and not an error.
        assert_eq!(usage_cost_from_completion_response(&json!({"usage": {"total_tokens": 9}})), 0.0);
        // A bool is not a number, exactly as Python's `isinstance` guard says.
        assert_eq!(usage_cost_from_completion_response(&json!({"usage": {"cost": true}})), 0.0);
    }

    // -- startup recovery --------------------------------------------------

    /// `test_startup_recovery.py`, all seven cases, as one decision table.
    #[test]
    fn startup_recovery_decides_by_status_dag_and_open_review_gate() {
        assert_eq!(recovery_action("pending", None, 0), Some(Recovery::Replan));
        assert_eq!(recovery_action("planning", None, 0), Some(Recovery::Replan));

        assert_eq!(
            recovery_action("approved", Some(r#"{"subagents": []}"#), 0),
            Some(Recovery::RequeueApproved)
        );
        assert_eq!(recovery_action("approved", None, 0), Some(Recovery::SkipApprovedWithoutDag));
        assert_eq!(
            recovery_action("approved", Some("   "), 0),
            Some(Recovery::SkipApprovedWithoutDag)
        );

        assert_eq!(recovery_action("running", Some("{}"), 0), Some(Recovery::RequeueRunning));
        assert_eq!(recovery_action("running", Some("{}"), 1), Some(Recovery::AlignReview));

        // Human gates and terminal rows are left alone — they are not even in
        // the query, and the action table says so too.
        for status in
            ["approval_required", "task_review_required", "completed", "failed", "cancelled"]
        {
            assert_eq!(recovery_action(status, Some("{}"), 0), None, "{status}");
        }
    }

    // -- team context ------------------------------------------------------

    #[test]
    fn the_team_snapshot_renders_the_roster_the_planner_was_trained_on() {
        let snapshot = json!({
            "team_template_id": 1,
            "name": "Research pod",
            "description": "  finds things  ",
            "color": "#abcdef",
            "roster": {"roles": [
                {"id": "lead", "name": "Lead", "description": "runs it", "modality": "text"},
                {"id": "b", "name": "Bravo", "description": "", "modality": "text", "parent_id": "lead"},
                {"id": "a", "name": "Alpha", "description": "", "modality": "text", "parent_id": "lead"},
            ]},
        })
        .to_string();

        let rendered = team_context_from_snapshot_json(Some(&snapshot)).unwrap();
        assert_eq!(
            rendered,
            "Team template: Research pod\n\
             Team description: finds things\n\
             Team color (UI hint): #abcdef\n\
             Preferred team roster (map subagent `role` names and responsibilities to these where sensible):\n\
             - Lead (id=lead): runs it\n\
             \x20 - Alpha (id=a)\n\
             \x20 - Bravo (id=b)"
        );

        // Nothing usable in, nothing out — the planner just gets no roster.
        assert_eq!(team_context_from_snapshot_json(None), None);
        assert_eq!(team_context_from_snapshot_json(Some("   ")), None);
        assert_eq!(team_context_from_snapshot_json(Some("not json")), None);
        assert_eq!(team_context_from_snapshot_json(Some(r#"{"name": "x"}"#)), None);
    }

    #[test]
    fn a_role_cycle_in_a_snapshot_does_not_hang_the_depth_walk() {
        let parents: HashMap<&str, Option<&str>> =
            [("a", Some("b")), ("b", Some("a"))].into_iter().collect();
        assert_eq!(role_depth("a", &parents), 2);

        let roots: HashMap<&str, Option<&str>> =
            [("a", None), ("b", Some("a"))].into_iter().collect();
        assert_eq!(role_depth("a", &roots), 0);
        assert_eq!(role_depth("b", &roots), 1);
    }
}
