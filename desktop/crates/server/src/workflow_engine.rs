//! Workflow execution: template resolution, step execution, the interval
//! scheduler. Port of `app/workflows/engine.py`.
//!
//! Steps run strictly top to bottom and the first failure stops the run; the
//! steps after it are recorded as `skipped` so a reader can tell "did not run"
//! from "ran and passed".
//!
//! The scheduler moved with the engine deliberately. Two servers polling the
//! same `workflows` table would each fire every due workflow, so `upstream.rs`
//! starts the Python child with `AGENT_PLATFORM_WORKFLOW_SCHEDULER=0` — its loop
//! and this one cannot both be live.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::wire::sql_now;
use crate::AppState;

const MAX_OUTPUT_BYTES: usize = 65536;
const DEFAULT_HTTP_TIMEOUT: f64 = 30.0;
const MAX_HTTP_TIMEOUT: f64 = 120.0;
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// A step failed. The message is what lands in the run's `error`, so it reads
/// the way Python's does.
#[derive(Debug)]
pub struct StepError(String);

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn step_error(message: impl Into<String>) -> StepError {
    StepError(message.into())
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// `{{ trigger.body.name }}` → the trimmed path, if the whole string is one
/// template. Hand-scanned rather than regex: the grammar is two braces around
/// `[A-Za-z0-9_.-]`.
fn whole_template(value: &str) -> Option<&str> {
    let inner = value.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    (!inner.is_empty() && inner.chars().all(is_path_char)).then_some(inner)
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn resolve_path(path: &str, ctx: &Value) -> Result<Value, StepError> {
    let mut current = ctx;
    for part in path.split('.') {
        current = match current {
            Value::Object(map) => map
                .get(part)
                .ok_or_else(|| step_error(format!("template path not found: {path}")))?,
            Value::Array(items) => part
                .parse::<usize>()
                .ok()
                .and_then(|i| items.get(i))
                .ok_or_else(|| step_error(format!("template path not found: {path}")))?,
            _ => return Err(step_error(format!("template path not found: {path}"))),
        };
    }
    Ok(current.clone())
}

/// Python's `str(value)` for a template embedded in a longer string: a bare
/// string interpolates without quotes, everything else uses its JSON form —
/// except Python's `True`/`None` spelling, which is what a caller building a
/// URL would actually see.
fn stringify(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

/// Substitute `{{path}}` references. A string that is exactly one template
/// resolves to the referenced value and keeps its type; a template embedded in a
/// longer string is stringified.
pub fn resolve_templates(value: &Value, ctx: &Value) -> Result<Value, StepError> {
    match value {
        Value::String(text) => {
            if let Some(path) = whole_template(text) {
                return resolve_path(path, ctx);
            }
            Ok(Value::String(substitute(text, ctx)?))
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                out.insert(key.clone(), resolve_templates(item, ctx)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => {
            items.iter().map(|item| resolve_templates(item, ctx)).collect::<Result<_, _>>().map(Value::Array)
        }
        other => Ok(other.clone()),
    }
}

fn substitute(text: &str, ctx: &Value) -> Result<String, StepError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let (before, from_open) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = from_open.find("}}") else {
            // An unclosed `{{` is literal text, as it is to the regex.
            out.push_str(from_open);
            return Ok(out);
        };
        let path = from_open[2..end].trim();
        if path.is_empty() || !path.chars().all(is_path_char) {
            out.push_str(&from_open[..end + 2]);
        } else {
            out.push_str(&stringify(&resolve_path(path, ctx)?));
        }
        rest = &from_open[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Step execution
// ---------------------------------------------------------------------------

async fn execute_http(http: &reqwest::Client, params: &Value) -> Result<Value, StepError> {
    // ponytail: a missing `url` or a non-numeric `timeout_seconds` is a step
    // failure here and an unhandled 500 in Python. The run records which step
    // and why either way; a crashed request does not.
    let url = params
        .get("url")
        .map(stringify)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| step_error("unsupported url scheme: None"))?;
    let lowered = url.to_ascii_lowercase();
    if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
        return Err(step_error(format!("unsupported url scheme: {url}")));
    }

    // The lower bound is not Python's — it has none — but `Duration` panics on a
    // negative, and a step is allowed to carry any number a user typed.
    let timeout = params
        .get("timeout_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_HTTP_TIMEOUT)
        .clamp(0.001, MAX_HTTP_TIMEOUT);
    let method = params
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| step_error(format!("unsupported http method: {method}")))?;

    let mut request = http.request(method, &url).timeout(Duration::from_secs_f64(timeout));
    if let Some(headers) = params.get("headers").and_then(Value::as_object) {
        for (name, value) in headers {
            request = request.header(name, stringify(value));
        }
    }
    request = match params.get("body") {
        None | Some(Value::Null) => request,
        Some(body @ (Value::Object(_) | Value::Array(_))) => request.json(body),
        Some(other) => request.body(stringify(other)),
    };

    let response = request.send().await.map_err(|e| step_error(format!("http request failed: {e}")))?;
    let status = response.status().as_u16();
    let raw = response.bytes().await.map_err(|e| step_error(format!("http request failed: {e}")))?;
    let truncated = &raw[..raw.len().min(MAX_OUTPUT_BYTES)];
    let body: Value = serde_json::from_slice(truncated)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(truncated).into_owned()));

    let output = json!({ "status": status, "body": body });
    if status >= 400 {
        let rendered: String = python_json(&output["body"]).chars().take(500).collect();
        return Err(step_error(format!("http {status} from {url}: {rendered}")));
    }
    Ok(output)
}

/// `json.dumps` spacing. Python's default separators are `", "` and `": "`, and
/// this rendering is the user-visible `error` stored on a failed step — so it is
/// part of the contract, not a formatting preference.
struct PythonJson;

impl serde_json::ser::Formatter for PythonJson {
    fn begin_array_value<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if first { Ok(()) } else { writer.write_all(b", ") }
    }

    fn begin_object_key<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if first { Ok(()) } else { writer.write_all(b", ") }
    }

    fn begin_object_value<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        writer.write_all(b": ")
    }
}

fn python_json(value: &Value) -> String {
    use serde::Serialize;
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, PythonJson);
    match value.serialize(&mut serializer) {
        Ok(()) => String::from_utf8(buffer).unwrap_or_else(|_| value.to_string()),
        Err(_) => value.to_string(),
    }
}

/// A registered server-executed action, resolved to the endpoint it POSTs to.
/// The `actions` table stays Python's; this only reads it.
async fn execute_action(state: &AppState, params: &Value) -> Result<Value, StepError> {
    let set_id = params.get("action_set_id").and_then(Value::as_i64);
    let action_id = params.get("action_id").map(stringify).unwrap_or_default();
    let label = format!(
        "set {} / {}",
        params.get("action_set_id").map(stringify).unwrap_or_else(|| "None".into()),
        action_id
    );

    let row: Option<(String, Option<String>)> = match set_id {
        Some(set_id) => sqlx::query_as(
            "SELECT execution_mode, endpoint FROM actions WHERE set_id = ? AND action_id = ?",
        )
        .bind(set_id)
        .bind(&action_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| step_error(format!("action lookup failed: {e}")))?,
        None => None,
    };

    let Some((execution_mode, endpoint)) = row else {
        return Err(step_error(format!("action not found: {label}")));
    };
    let endpoint = endpoint.filter(|e| !e.is_empty());
    let Some(endpoint) = endpoint.filter(|_| execution_mode == "server") else {
        return Err(step_error(format!(
            "action '{action_id}' is not server-executable \
             (needs execution_mode 'server' and an endpoint)"
        )));
    };

    let arguments = match params.get("arguments") {
        Some(v @ Value::Object(_)) => v.clone(),
        _ => json!({}),
    };
    execute_http(&state.http, &json!({ "url": endpoint, "method": "POST", "body": arguments })).await
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// Run every step of `workflow_id`, writing the run row as it goes. Returns the
/// new run's id.
pub async fn execute_workflow(
    state: &AppState,
    workflow_id: i64,
    steps_json: &str,
    input: Value,
    trigger: &str,
) -> Result<i64, sqlx::Error> {
    let steps = match serde_json::from_str::<Value>(steps_json) {
        Ok(Value::Array(steps)) => steps,
        _ => Vec::new(),
    };

    let input_json = match &input {
        Value::Object(map) if !map.is_empty() => Some(input.to_string()),
        // `set_input` stores nothing for an empty body, and `get_input` reads
        // that back as `{}`.
        _ => None,
    };
    let run_id: i64 = sqlx::query_scalar(
        "INSERT INTO workflow_runs (workflow_id, trigger, status, input_json, steps_json, started_at) \
         VALUES (?, ?, 'running', ?, '[]', ?) RETURNING id",
    )
    .bind(workflow_id)
    .bind(trigger)
    .bind(&input_json)
    .bind(sql_now())
    .fetch_one(&state.pool)
    .await?;

    let mut ctx = json!({ "trigger": { "body": input }, "steps": {} });
    let mut results: Vec<Value> = Vec::new();
    let mut failed: Option<String> = None;

    for step in &steps {
        let step_id = step.get("id").map(stringify).unwrap_or_default();
        let started = Instant::now();
        let outcome = run_step(state, step, &ctx).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match outcome {
            Ok(output) => {
                ctx["steps"][&step_id] = json!({ "output": output });
                results.push(json!({
                    "id": step_id,
                    "status": "succeeded",
                    "output": output,
                    "duration_ms": duration_ms,
                }));
            }
            Err(e) => {
                failed = Some(format!("step '{step_id}': {e}"));
                results.push(json!({
                    "id": step_id,
                    "status": "failed",
                    "error": e.to_string(),
                    "duration_ms": duration_ms,
                }));
                break;
            }
        }
    }

    if failed.is_some() {
        // Declaration order, where Python iterates a set difference and so has
        // no order at all. Same ids, and this one reads like the workflow.
        let ran: Vec<String> = results.iter().map(|r| stringify(&r["id"])).collect();
        for step in &steps {
            let step_id = step.get("id").map(stringify).unwrap_or_default();
            if !ran.contains(&step_id) {
                results.push(json!({ "id": step_id, "status": "skipped" }));
            }
        }
    }

    sqlx::query(
        "UPDATE workflow_runs SET status = ?, error = ?, steps_json = ?, finished_at = ? WHERE id = ?",
    )
    .bind(if failed.is_some() { "failed" } else { "succeeded" })
    .bind(&failed)
    .bind(Value::Array(results).to_string())
    .bind(sql_now())
    .bind(run_id)
    .execute(&state.pool)
    .await?;

    Ok(run_id)
}

async fn run_step(state: &AppState, step: &Value, ctx: &Value) -> Result<Value, StepError> {
    let params = match step.get("params") {
        Some(params @ Value::Object(_)) => resolve_templates(params, ctx)?,
        _ => json!({}),
    };
    match step.get("type").and_then(Value::as_str) {
        Some("http") => execute_http(&state.http, &params).await,
        Some("action") => execute_action(state, &params).await,
        other => Err(step_error(format!(
            "unknown step type: {}",
            other.map(str::to_string).unwrap_or_else(|| "None".into())
        ))),
    }
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Poll for due workflows and run them. Off with
/// `AGENT_PLATFORM_WORKFLOW_SCHEDULER=0`, the same switch Python reads.
pub fn spawn_scheduler(state: Arc<AppState>) {
    if crate::env_opt("AGENT_PLATFORM_WORKFLOW_SCHEDULER").as_deref() == Some("0") {
        return;
    }
    // Attached to a server we did not start (`AGENT_PLATFORM_UPSTREAM`): its
    // scheduler is already running and we cannot switch it off, so staying quiet
    // is the only way not to fire everything twice.
    if state.upstream.child_alive().is_none() {
        eprintln!(
            "[agent-platformd] workflow scheduler not started: attached to an upstream that owns it"
        );
        return;
    }
    eprintln!("[agent-platformd] workflow scheduler started (poll every {POLL_INTERVAL:?})");
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Err(e) = run_due_workflows(&state).await {
                eprintln!("[agent-platformd] workflow scheduler tick failed: {e}");
            }
        }
    });
}

async fn run_due_workflows(state: &AppState) -> Result<(), sqlx::Error> {
    let now = sql_now();
    let due: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT id, steps_json, interval_seconds FROM workflows \
         WHERE enabled = 1 AND interval_seconds IS NOT NULL \
           AND next_run_at IS NOT NULL AND next_run_at <= ?",
    )
    .bind(&now)
    .fetch_all(&state.pool)
    .await?;

    // Advance before executing, so a workflow that crashes cannot tight-loop.
    for (id, _, interval_seconds) in &due {
        let next = chrono::Utc::now().naive_utc() + chrono::Duration::seconds(*interval_seconds);
        sqlx::query("UPDATE workflows SET next_run_at = ? WHERE id = ?")
            .bind(next.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
            .bind(id)
            .execute(&state.pool)
            .await?;
    }

    for (id, steps_json, _) in due {
        match execute_workflow(state, id, &steps_json, json!({}), "schedule").await {
            Ok(run_id) => eprintln!("[agent-platformd] scheduled workflow {id} finished (run {run_id})"),
            Err(e) => eprintln!("[agent-platformd] scheduled workflow {id} crashed: {e}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Value {
        json!({
            "trigger": { "body": { "name": "ada", "count": 3, "on": true } },
            "steps": { "fetch": { "output": { "status": 200, "body": { "items": [{"id": 7}] } } } }
        })
    }

    #[test]
    fn a_lone_template_keeps_the_referenced_type() {
        let ctx = ctx();
        assert_eq!(resolve_templates(&json!("{{trigger.body.count}}"), &ctx).unwrap(), json!(3));
        assert_eq!(resolve_templates(&json!("{{ trigger.body.on }}"), &ctx).unwrap(), json!(true));
        assert_eq!(
            resolve_templates(&json!("{{steps.fetch.output.body.items.0.id}}"), &ctx).unwrap(),
            json!(7)
        );

        // Embedded in a longer string, it is stringified instead.
        assert_eq!(
            resolve_templates(&json!("hi {{trigger.body.name}} x{{trigger.body.count}}"), &ctx)
                .unwrap(),
            json!("hi ada x3")
        );
    }

    #[test]
    fn templates_recurse_and_a_missing_path_fails_the_step() {
        let ctx = ctx();
        let params = json!({
            "url": "https://x.test/{{trigger.body.name}}",
            "headers": { "X-Count": "{{trigger.body.count}}" },
            "body": { "items": ["{{steps.fetch.output.status}}"] },
        });
        let resolved = resolve_templates(&params, &ctx).unwrap();
        assert_eq!(resolved["url"], json!("https://x.test/ada"));
        // A header whose value is exactly one template keeps the number; it is
        // stringified when the request is built, not when it is resolved.
        assert_eq!(resolved["headers"]["X-Count"], json!(3));
        assert_eq!(resolved["body"]["items"][0], json!(200));

        let err = resolve_templates(&json!("{{trigger.body.nope}}"), &ctx).unwrap_err();
        assert_eq!(err.to_string(), "template path not found: trigger.body.nope");
        // Indexing past the end of a list is the same failure.
        assert!(resolve_templates(&json!("{{steps.fetch.output.body.items.9.id}}"), &ctx).is_err());
    }

    #[test]
    fn failed_step_bodies_render_with_pythons_spacing() {
        // This string is stored on the run and shown to the user, so the
        // separators are contract, not taste.
        assert_eq!(python_json(&json!({"detail": "Not Found"})), r#"{"detail": "Not Found"}"#);
        assert_eq!(python_json(&json!([1, 2])), "[1, 2]");
        assert_eq!(python_json(&json!({"a": [{"b": 1}]})), r#"{"a": [{"b": 1}]}"#);
        // Escaping is serde's, so a separator inside a string stays untouched.
        assert_eq!(python_json(&json!({"a": "x, y"})), r#"{"a": "x, y"}"#);
    }

    #[test]
    fn text_that_only_looks_like_a_template_is_left_alone() {
        let ctx = ctx();
        for literal in ["{{ }}", "{{not a path}}", "{{unclosed", "plain"] {
            assert_eq!(
                resolve_templates(&json!(literal), &ctx).unwrap(),
                json!(literal),
                "{literal}"
            );
        }
    }
}
