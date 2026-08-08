//! The action orchestrator — `app/action_orchestrator/`.
//!
//! Started as just the part `todos agent/step` calls (`registry.list_actions`
//! and `engine.decide_actions`, with the router proxied); the eleven routes
//! (`/action-sets`, `/sessions`, `/decide`) landed here on 2026-08-07 and the
//! domain is whole.
//!
//! **The two text-parsing fallbacks are the point.** A model that answers the
//! tool-call prompt with prose, or with one JSON object, still has to produce
//! actions — and this screen mostly runs local models, which is exactly when the
//! tool protocol gets ignored. `parse_decision_response` and
//! `parse_actions_from_text` are pure functions over the model's reply, and they
//! are the thing that regresses silently, so they carry the tests below.
//!
//! Two shapes carried over from Python on purpose:
//!
//! - **`decide_actions` never fails.** Python wraps the whole body in
//!   `except Exception` and returns `([], f"Error during decision: {e}")`, so an
//!   unreachable proxy is a **200** whose `thought` explains the failure, not a
//!   502. The wording of that message is Python's exception text and cannot
//!   match; the status, the empty action list and the `Error during decision: `
//!   prefix do.
//! - **Errors that Python raises *inside* the parse propagate.** A confidence of
//!   `1.5` fails pydantic's `le=1.0` and takes the whole step into that same
//!   error path; a `<actions>` block that is not a list raises before any action
//!   is built. Those are modelled as `Err(String)` here for the same reason —
//!   the alternative is proposing an action Python refuses.
//!
//! Usage accounting is dropped: `decide_actions` returns an `LlmUsageOut` that
//! `agent_bridge.agent_step` immediately discards (`planned, thought, _`).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::chat_usage::{coerce_int, LlmStepUsageOut};
use crate::dag_schema::sanitize_llm_model_alias;
use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::wire::{defaulted_str, lax_bool, optional_str, parse_body, required_str, sql_now};
use crate::todos::py_repr;
use crate::AppState;

/// `action_orchestrator.models.Action`, the four columns the engine reads.
#[derive(FromRow)]
pub(crate) struct ActionRow {
    pub action_id: String,
    pub name: String,
    pub description: String,
    pub parameters_json: String,
}

/// `registry.list_actions`. No `ORDER BY`, because SQLAlchemy emits none either
/// and the tool order the model sees has to be the same arbitrary rowid order.
pub(crate) async fn list_actions(
    state: &AppState,
    set_id: i64,
) -> Result<Vec<ActionRow>, ApiError> {
    Ok(sqlx::query_as(&crate::db::sql(
        "SELECT action_id, name, description, parameters_json FROM actions WHERE set_id = ?", state.backend)
    )
    .bind(set_id)
    .fetch_all(&state.any)
    .await?)
}

/// `schemas.PlannedAction`. Field order is pydantic's declaration order, which
/// is what the response body carries — serialize it directly, never through
/// `serde_json::to_value`, or the keys sort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlannedAction {
    pub action_id: String,
    pub name: String,
    pub parameters: Map<String, Value>,
    pub confidence: f64,
    pub reasoning: Option<String>,
}

// ---------------------------------------------------------------------------
// The prompt
// ---------------------------------------------------------------------------

/// `engine.build_action_tools`.
pub(crate) fn build_action_tools(actions: &[ActionRow]) -> Vec<Value> {
    actions
        .iter()
        .map(|action| {
            // `Action.get_parameters` swallows a decode error and returns `{}`.
            // A stored `5` or `null` is *not* an error there — it is returned as
            // it is and then fails the `if params` truthiness test below, same
            // as `{}` does.
            let params =
                serde_json::from_str::<Value>(&action.parameters_json).unwrap_or_else(|_| json!({}));
            let params = if py_truthy(&params) {
                params
            } else {
                json!({"type": "object", "properties": {}})
            };
            json!({
                "type": "function",
                "function": {
                    "name": action.action_id,
                    "description": action.description,
                    "parameters": params,
                },
            })
        })
        .collect()
}

/// `engine.build_system_message`, verbatim. Every newline in it is load-bearing:
/// it is the prompt a planner profile was tuned against.
fn build_system_message() -> &'static str {
    "You are an intelligent action planner. Your job is to analyze the user's goal and context, then select the appropriate actions from the available tools to accomplish that goal.

Guidelines:
1. Analyze the goal and context carefully
2. Select only actions that are relevant and necessary
3. Set appropriate parameters for each action based on the context
4. If multiple actions are needed, they will be called in sequence
5. If no action is appropriate, indicate completion
6. Provide clear reasoning for your choices
7. For ask_clarifying_questions, always pass a non-empty \"questions\" array of specific strings
   (never call it with empty parameters). Optionally pass \"fields\" with id, label, kind
   (boolean | single_select | multi_select | text | textarea), options for selects, and required.
   Put choices in parentheses in the question or in options — the UI will show pickers.
   If user_domain_profiles already has the needed fields, prefer create_item / break_down_task
   instead of asking again.

You can call multiple tools if needed to accomplish complex goals."
}

/// `engine.build_user_message`, minus the `history` block: `agent_step` always
/// passes `history=None`, and the callers that pass one (`/decide`, `/sessions`)
/// stay in Python.
///
/// The context is `json.dumps(..., indent=2)` here — **not** the `str(dict)`
/// `agent/chat` interpolates — so this one is genuinely JSON.
fn build_user_message(goal: &str, context: &Map<String, Value>, history: &[Value]) -> String {
    let mut parts = vec![format!("Goal: {goal}")];

    if !context.is_empty() {
        let conversation = context.get("conversation_history");
        let for_json: Map<String, Value> = context
            .iter()
            .filter(|(key, _)| key.as_str() != "conversation_history")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        parts.push(format!("Context: {}", json_dumps_indent2(&Value::Object(for_json))));

        if let Some(Value::Array(turns)) = conversation {
            if !turns.is_empty() {
                parts.push("\nConversation so far:".to_string());
                let tail = turns.len().saturating_sub(12);
                for turn in &turns[tail..] {
                    let Some(fields) = turn.as_object() else { continue };
                    // `if text:` — a falsy content drops the whole turn.
                    let Some(content) = fields.get("content").filter(|v| py_truthy(v)) else {
                        continue;
                    };
                    // `turn.get("role", "user")` defaults on *absence* only, so
                    // an explicit `null` renders as Python's `str(None)`.
                    let role = fields.get("role").map_or("user".to_string(), py_display);
                    parts.push(format!("{role}: {}", py_display(content)));
                }
            }
        }
    }

    // Only the session routes pass this: what the client already ran, and what
    // came back. A failed action is named as failed rather than summarised.
    if !history.is_empty() {
        parts.push("\nPrevious actions and results:".to_string());
        for entry in history {
            let action_id = entry.get("action_id").map_or("unknown".to_string(), py_display);
            match entry.get("error").filter(|v| py_truthy(v)) {
                Some(error) => parts.push(format!("- {action_id}: FAILED - {}", py_display(error))),
                None => {
                    let result = entry.get("result").cloned().unwrap_or_else(|| json!({}));
                    let rendered = json_dumps_indent2(&result);
                    let truncated: String = rendered.chars().take(200).collect();
                    parts.push(format!("- {action_id}: {truncated}"));
                }
            }
        }
    }

    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

/// `engine.decide_actions`: one tool-enabled completion, then two fallbacks for
/// a model that answered in prose.
///
/// Returns `(planned actions, thought, usage steps)` — the third element is
/// `LlmUsageOut.steps` from Python's return, which every caller here
/// immediately unpacks with `.extend()` rather than holding the merged whole;
/// `agent_step` (`todos.rs`) discards it, `assistant.rs`'s turn generation
/// does not. Never `Err` — see the module docs.
pub(crate) async fn decide_actions(
    state: &AppState,
    goal: &str,
    context: &Map<String, Value>,
    actions: &[ActionRow],
    history: &[Value],
    llm_model: &str,
) -> (Vec<PlannedAction>, Option<String>, Vec<LlmStepUsageOut>) {
    if actions.is_empty() {
        return (
            Vec::new(),
            Some("No actions available in the action set.".to_string()),
            Vec::new(),
        );
    }

    let tools = build_action_tools(actions);
    let messages = vec![
        json!({"role": "system", "content": build_system_message()}),
        json!({"role": "user", "content": build_user_message(goal, context, history)}),
    ];

    match decide(state, &messages, tools, actions, llm_model).await {
        Ok(decision) => decision,
        Err((message, steps)) => (Vec::new(), Some(format!("Error during decision: {message}")), steps),
    }
}

/// The body of Python's `try:`. Anything that raises there is an `Err` here,
/// carrying whatever usage steps had already accumulated — Python's `except`
/// wraps the same `usage_steps` list the `try` was building.
async fn decide(
    state: &AppState,
    messages: &[Value],
    tools: Vec<Value>,
    actions: &[ActionRow],
    llm_model: &str,
) -> Result<(Vec<PlannedAction>, Option<String>, Vec<LlmStepUsageOut>), (String, Vec<LlmStepUsageOut>)> {
    let mut steps: Vec<LlmStepUsageOut> = Vec::new();

    let (content, tool_calls, tokens, cost) =
        call_tool_proposals(state, messages, tools, llm_model).await.map_err(|e| (e, steps.clone()))?;
    steps.push(LlmStepUsageOut {
        total_tokens: tokens,
        cost_usd: cost,
        label: Some("decide_actions".to_string()),
        ..Default::default()
    });

    let planned = tool_calls_to_planned_actions(&tool_calls, actions).map_err(|e| (e, steps.clone()))?;
    if !planned.is_empty() {
        let thought = content.trim();
        let thought = if thought.is_empty() { None } else { Some(thought.to_string()) };
        return Ok((planned, thought, steps));
    }

    // Fallback 1: the model wrote its plan out instead of calling a tool.
    let parsed = parse_decision_response(&content).map_err(|e| (e, steps.clone()))?;
    if !parsed.actions.is_empty() {
        return Ok((parsed.actions, parsed.thought, steps));
    }

    // Fallback 2: ask again with no tools at all. Some local models only produce
    // usable text once the tool schema is out of the prompt.
    let (reasoning, tokens2, cost2) =
        call_llm(state, messages, llm_model).await.map_err(|e| (e, steps.clone()))?;
    steps.push(LlmStepUsageOut {
        total_tokens: tokens2,
        cost_usd: cost2,
        label: Some("decide_actions_fallback".to_string()),
        ..Default::default()
    });
    let parsed = parse_decision_response(&reasoning).map_err(|e| (e, steps.clone()))?;
    Ok((parsed.actions, parsed.thought, steps))
}

/// `llm_client.call_llm_tool_proposals` without the loopback HTTP hop.
///
/// `complete_internal` already carries the status the `/v1` route would have
/// answered with, and every non-200 becomes an `Err` — which is what Python's
/// `LLMRequestError` does from inside the `try`. Only the message text differs
/// (Python's includes a truncated upstream body), and it is only ever read as
/// the `thought` of a failed step.
async fn call_tool_proposals(
    state: &AppState,
    messages: &[Value],
    tools: Vec<Value>,
    llm_model: &str,
) -> Result<(String, Vec<Value>, i64, f64), String> {
    let (fitted, _) = crate::context_budget::fit_chat_messages_for_request(messages.to_vec());
    let mut payload = Map::new();
    payload.insert("messages".into(), Value::Array(fitted));
    payload.insert("temperature".into(), json!(0.7));
    payload.insert("max_tokens".into(), json!(crate::context_budget::max_output_tokens_default()));
    payload.insert("tools".into(), Value::Array(tools));
    payload.insert("tool_choice".into(), json!("auto"));
    if let Some(model) = sanitize_llm_model_alias(llm_model) {
        payload.insert("model".into(), json!(model));
    }

    let data = crate::llm::complete_internal(state, payload).await.map_err(|e| e.message)?;
    // `data["choices"][0]["message"]` — a `KeyError`/`IndexError` in Python.
    let message = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| "'choices'".to_string())?;

    let content = message.get("content").filter(|v| py_truthy(v)).map_or(String::new(), py_display);
    let tool_calls = match message.get("tool_calls") {
        Some(Value::Array(calls)) => calls.clone(),
        _ => Vec::new(),
    };
    // `int(usage.get("total_tokens", 0) or 0)` — only the total, never the
    // prompt/completion split, which is why `decide()`'s steps below always
    // carry zero for those two fields.
    let tokens = coerce_int(data.get("usage").and_then(|u| u.get("total_tokens")));
    let cost = crate::executor::usage_cost_from_completion_response(&data);
    Ok((content, tool_calls, tokens, cost))
}

/// `llm_client.call_llm`: same call with no `tools`, so the model has to answer
/// in text. `content` may come back `null`, which Python turns into `""`.
async fn call_llm(
    state: &AppState,
    messages: &[Value],
    llm_model: &str,
) -> Result<(String, i64, f64), String> {
    let (fitted, _) = crate::context_budget::fit_chat_messages_for_request(messages.to_vec());
    let mut payload = Map::new();
    payload.insert("messages".into(), Value::Array(fitted));
    payload.insert("temperature".into(), json!(0.7));
    payload.insert("max_tokens".into(), json!(crate::context_budget::max_output_tokens_default()));
    if let Some(model) = sanitize_llm_model_alias(llm_model) {
        payload.insert("model".into(), json!(model));
    }

    let data = crate::llm::complete_internal(state, payload).await.map_err(|e| e.message)?;
    let content = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .ok_or_else(|| "'choices'".to_string())?;
    let content = content.as_str().map(str::to_string).unwrap_or_default();
    let tokens = coerce_int(data.get("usage").and_then(|u| u.get("total_tokens")));
    let cost = crate::executor::usage_cost_from_completion_response(&data);
    Ok((content, tokens, cost))
}

/// `engine.tool_calls_to_planned_actions`.
///
/// An id the action set does not contain is dropped — this is the one path that
/// checks, which is why a JSON-parsed action (below) keeps `action_id` only and
/// never a guessed display name.
fn tool_calls_to_planned_actions(
    tool_calls: &[Value],
    actions: &[ActionRow],
) -> Result<Vec<PlannedAction>, String> {
    let mut planned = Vec::new();
    for call in tool_calls {
        // `tc.get(...)` on a non-dict is an `AttributeError` in Python.
        let call = call
            .as_object()
            .ok_or_else(|| "'str' object has no attribute 'get'".to_string())?;
        let function = call.get("function").and_then(Value::as_object);
        let name = match function.and_then(|f| f.get("name")).filter(|v| py_truthy(v)) {
            None => String::new(),
            Some(Value::String(name)) => name.trim().to_string(),
            // `(fn.get("name") or "").strip()` on a non-string raises too.
            Some(_) => return Err("object has no attribute 'strip'".to_string()),
        };
        let Some(action) = actions.iter().find(|a| a.action_id == name) else { continue };

        let parameters = match function.and_then(|f| f.get("arguments")) {
            Some(Value::String(raw)) if !raw.trim().is_empty() => {
                // A malformed argument blob is `{}`, not a failure: the model
                // proposed the action, and the user still gets to see it.
                serde_json::from_str::<Value>(raw)
                    .ok()
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default()
            }
            Some(Value::Object(args)) => args.clone(),
            _ => Map::new(),
        };
        planned.push(PlannedAction {
            action_id: name,
            name: action.name.clone(),
            parameters,
            confidence: 0.9,
            reasoning: None,
        });
    }
    Ok(planned)
}

// ---------------------------------------------------------------------------
// The text fallbacks
// ---------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq)]
struct Decision {
    thought: Option<String>,
    actions: Vec<PlannedAction>,
}

/// `engine._decision_from_json`: a model that answers the tool-call prompt with
/// one JSON object instead of calling a tool.
///
/// Without this the object matches neither `<reasoning>` nor `Thought:`, so the
/// thought falls back to the first 200 characters of the raw response — a JSON
/// fence truncated mid-sentence — and that string is what the review banner
/// shows the user. Its `actions` are lost in the same step.
fn decision_from_json(response: &str) -> Result<Option<Decision>, String> {
    let Ok(data) = serde_json::from_str::<Value>(&strip_code_fences(response)) else {
        return Ok(None);
    };
    let Some(data) = data.as_object() else { return Ok(None) };

    let thought = data
        .get("reasoning")
        .filter(|v| py_truthy(v))
        .or_else(|| data.get("thought").filter(|v| py_truthy(v)))
        .map(|v| py_display(v).trim().to_string());

    let mut actions = Vec::new();
    match data.get("actions") {
        // `data.get("actions") or []` — every falsy value is an empty list.
        None => {}
        Some(value) if !py_truthy(value) => {}
        // Iterating a dict or a string yields keys/characters, and the
        // `isinstance(a, dict)` guard then skips every one of them.
        Some(Value::Object(_)) | Some(Value::String(_)) => {}
        Some(Value::Array(items)) => {
            for item in items {
                let Some(item) = item.as_object() else { continue };
                // `action_id` only, never the display name: unlike the tool-call
                // path these are not checked against the action set, so a
                // guessed id would travel as a real proposal.
                let action_id = item
                    .get("action_id")
                    .filter(|v| py_truthy(v))
                    .map_or(String::new(), py_display)
                    .trim()
                    .to_string();
                if action_id.is_empty() {
                    continue;
                }
                let parameters =
                    item.get("parameters").and_then(Value::as_object).cloned().unwrap_or_default();
                let confidence = match item.get("confidence").filter(|v| py_truthy(v)) {
                    None => 0.9,
                    Some(value) => py_float(value)?,
                };
                if !(0.0..=1.0).contains(&confidence) {
                    // pydantic's `ge=0.0, le=1.0`. Python raises here and the
                    // whole step becomes an error thought, so this cannot be
                    // softened into a clamp without proposing an action Python
                    // refused.
                    return Err(format!(
                        "1 validation error for PlannedAction\nconfidence\n  Input should be less than or equal to 1 (got {confidence})"
                    ));
                }
                let name = item
                    .get("name")
                    .filter(|v| py_truthy(v))
                    .map_or_else(|| action_id.clone(), py_display);
                let reasoning = match item.get("reasoning") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(text)) => Some(text.clone()),
                    Some(_) => {
                        return Err(
                            "1 validation error for PlannedAction\nreasoning\n  Input should be a valid string".to_string()
                        )
                    }
                };
                actions.push(PlannedAction { action_id, name, parameters, confidence, reasoning });
            }
        }
        // `for a in 5:` — not iterable.
        Some(_) => return Err("'int' object is not iterable".to_string()),
    }

    Ok(Some(Decision { thought, actions }))
}

/// `engine.parse_decision_response`: JSON object, then `<reasoning>`/`Thought:`
/// for the thought, then `<actions>` or bare `Action:` lines for the actions.
fn parse_decision_response(response: &str) -> Result<Decision, String> {
    if let Some(structured) = decision_from_json(response)? {
        if structured.thought.as_deref().is_some_and(|t| !t.is_empty())
            || !structured.actions.is_empty()
        {
            return Ok(structured);
        }
    }

    let mut thought: Option<String> = None;
    let mut actions: Vec<PlannedAction> = Vec::new();

    if let Some(inner) = between(response, "<reasoning>", "</reasoning>") {
        thought = Some(inner.trim().to_string());
    } else if response.contains("Thought:") {
        let lines: Vec<&str> = response.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.starts_with("Thought:") {
                continue;
            }
            // `.replace` with no count: every occurrence on the line goes.
            let mut text = line.replace("Thought:", "").trim().to_string();
            for next in &lines[i + 1..] {
                if next.trim().starts_with("Action") {
                    break;
                }
                text.push('\n');
                text.push_str(next);
            }
            thought = Some(text);
            break;
        }
    }

    if let Some(inner) = between(response, "<actions>", "</actions>") {
        // A parse failure here is logged and swallowed by Python; a *type*
        // failure inside the loop is not, and neither is an element that is not
        // a dict. `break` rather than discard, so a good action ahead of a bad
        // one survives exactly as it does there.
        if let Ok(data) = serde_json::from_str::<Value>(inner.trim()) {
            match data {
                Value::Array(items) => {
                    for item in &items {
                        let Some(item) = item.as_object() else {
                            return Err("'str' object has no attribute 'get'".to_string());
                        };
                        match planned_from_actions_block(item) {
                            Some(action) => actions.push(action),
                            None => break,
                        }
                    }
                }
                // Iterating these yields nothing at all, so no `AttributeError`.
                Value::Object(map) if map.is_empty() => {}
                Value::String(text) if text.is_empty() => {}
                Value::Object(_) | Value::String(_) => {
                    return Err("'str' object has no attribute 'get'".to_string())
                }
                _ => return Err("'int' object is not iterable".to_string()),
            }
        }
    }

    if actions.is_empty() {
        actions = parse_actions_from_text(response)?;
    }

    // The last resort is the head of the raw response, which is fine when the
    // model wrote prose and unusable when it wrote a data structure this parser
    // could not read: callers put this string in front of the user as the
    // assistant's own words. Better to have no thought than a broken-looking one.
    let fallback = if looks_like_machine_output(response) { "" } else { response.trim() };
    let thought = thought.filter(|t| !t.is_empty()).or_else(|| {
        // 200 *characters*, not bytes — Python slices code points.
        let head: String = fallback.chars().take(200).collect();
        if head.is_empty() {
            None
        } else {
            Some(head)
        }
    });
    Ok(Decision { thought, actions })
}

/// One entry of an `<actions>` block. `None` is pydantic's `ValidationError`,
/// which Python catches as a `ValueError` and turns into a `break`.
fn planned_from_actions_block(item: &Map<String, Value>) -> Option<PlannedAction> {
    let field = |key: &str| -> Option<String> {
        match item.get(key) {
            None => Some(String::new()),
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => None,
        }
    };
    let action_id = field("action_id")?;
    let name = field("name")?;
    let parameters = match item.get("parameters") {
        None => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return None,
    };
    let confidence = match item.get("confidence") {
        None => 0.9,
        // pydantic coerces a numeric string, and rejects a bool for a float.
        Some(Value::Number(n)) => n.as_f64()?,
        Some(Value::String(text)) => text.trim().parse::<f64>().ok()?,
        Some(_) => return None,
    };
    if !(0.0..=1.0).contains(&confidence) {
        return None;
    }
    let reasoning = match item.get("reasoning") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => return None,
    };
    Some(PlannedAction { action_id, name, parameters, confidence, reasoning })
}

/// `engine.parse_actions_from_text`: `Action: <id>` lines, then `key = value`
/// lines beneath each one.
///
/// `Action:` with nothing after it is an `IndexError` in Python (`.split()[0]`
/// on an empty list), so it is an `Err` here rather than a skipped line.
fn parse_actions_from_text(text: &str) -> Result<Vec<PlannedAction>, String> {
    let mut actions: Vec<PlannedAction> = Vec::new();
    let mut current: Option<PlannedAction> = None;

    for raw in text.split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lowered = line.to_lowercase();
        if lowered.starts_with("action:") || lowered.starts_with("- action:") {
            if let Some(action) = current.take() {
                actions.push(action);
            }
            let after = line.split_once(':').map(|(_, rest)| rest).unwrap_or_default();
            let Some(name) = after.split_whitespace().next() else {
                return Err("list index out of range".to_string());
            };
            current = Some(PlannedAction {
                action_id: name.to_string(),
                name: name.to_string(),
                parameters: Map::new(),
                confidence: 0.8,
                reasoning: None,
            });
        } else if let Some(action) = current.as_mut() {
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                action.parameters.insert(key.trim().to_string(), json!(value));
            }
        }
    }

    if let Some(action) = current {
        actions.push(action);
    }
    Ok(actions)
}

// ---------------------------------------------------------------------------
// `llm_text.py`
// ---------------------------------------------------------------------------

/// `llm_text.strip_code_fences`: unwrap a backtick-fenced block and drop a
/// leading `<think>` section.
///
/// Reasoning models (deepseek-r1, qwen3, …) prefix their answer with inline
/// deliberation; it is not the answer. The fence is what a model adds when it
/// has been asked for JSON and decided to be helpful about it.
fn strip_code_fences(text: &str) -> String {
    // `^\s*<think>.*?</think>` — anchored, so at most one substitution.
    let mut body = text;
    if let Some(rest) = text.trim_start().strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            body = &rest[end + "</think>".len()..];
        }
    }
    let body = body.trim();

    // `^```(?:[a-zA-Z0-9_-]+)?\s*(.*?)\s*```$`. The optional language tag is
    // greedy, so ```` ```abc``` ```` reads `abc` as the tag and the body as
    // empty — matching Python's backtracking order.
    let Some(rest) = body.strip_prefix("```") else { return body.to_string() };
    let Some(inner) = rest.strip_suffix("```") else { return body.to_string() };
    let tag_len = inner
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
        .count();
    inner[tag_len..].trim().to_string()
}

/// `llm_text.looks_like_machine_output`: is this JSON or a code fence rather
/// than prose? Used to decide whether a string may be shown to the user as the
/// assistant's own words.
fn looks_like_machine_output(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with("```")
        || trimmed.starts_with("<think>")
}

// ---------------------------------------------------------------------------
// Python-shaped odds and ends
// ---------------------------------------------------------------------------

/// The text between two markers, by first occurrence of each. Python's
/// `str.index` pair produces an empty string when the closing marker comes
/// first, rather than raising.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text.find(close)?;
    Some(if end >= start { &text[start..end] } else { "" })
}

/// Python truthiness for a JSON value.
pub(crate) fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python's `str()`: a string is itself, everything else is its `repr`.
pub(crate) fn py_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => py_repr(other),
    }
}

/// Python's `float()` on a JSON value, as it behaves inside `x or 0.9`.
fn py_float(value: &Value) -> Result<f64, String> {
    match value {
        Value::Number(n) => n.as_f64().ok_or_else(|| "invalid number".to_string()),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("could not convert string to float: {}", py_repr(value))),
        _ => Err("float() argument must be a string or a real number".to_string()),
    }
}

/// `json.dumps(value, indent=2)`.
///
/// serde_json's pretty form already uses a two-space indent and `": "`, which is
/// what Python emits once `indent` is set. The one difference is `ensure_ascii`,
/// which is on by default there and absent here — and since every structural
/// character in the output is ASCII, escaping the whole rendered string is the
/// same thing as escaping each string literal in it.
///
/// **Key order is serde's, which is alphabetical; Python keeps insertion
/// order.** This is a prompt, not a stored artifact or a response body, and the
/// nested objects (item metadata, the caller's own `context`) arrive from
/// `serde_json` already sorted, so ordering the top level by hand would fix one
/// level of four. Documented rather than fought.
fn json_dumps_indent2(value: &Value) -> String {
    ensure_ascii(&serde_json::to_string_pretty(value).unwrap_or_default())
}

fn ensure_ascii(text: &str) -> String {
    if text.is_ascii() && !text.contains('\u{7f}') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut units = [0u16; 2];
    for ch in text.chars() {
        // DEL is ASCII but outside Python's printable range, so it escapes too.
        if ch.is_ascii() && ch != '\u{7f}' {
            out.push(ch);
            continue;
        }
        // Astral chars are a surrogate pair in Python's output, which is what
        // UTF-16 units give us.
        for unit in ch.encode_utf16(&mut units) {
            out.push_str(&format!("\\u{:04x}", *unit));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

pub fn routes() -> Router<Arc<AppState>> {
    const BASE: &str = "/api/v1";
    Router::new()
        .route(&format!("{BASE}/action-sets"), get(list_sets).post(create_set))
        .route(
            &format!("{BASE}/action-sets/{{set_id}}"),
            get(get_set).put(update_set).delete(delete_set),
        )
        .route(
            &format!("{BASE}/action-sets/{{set_id}}/actions"),
            get(list_set_actions).post(add_action),
        )
        .route(
            &format!("{BASE}/action-sets/{{set_id}}/actions/{{action_id}}"),
            get(get_action_detail).put(update_action_detail).delete(delete_action_endpoint),
        )
        .route(&format!("{BASE}/sessions"), post(create_session))
        .route(&format!("{BASE}/sessions/{{session_id}}"), get(get_session))
        .route(&format!("{BASE}/sessions/{{session_id}}/steps"), post(request_step))
        .route(&format!("{BASE}/sessions/{{session_id}}/results"), post(submit_result))
        .route(&format!("{BASE}/sessions/{{session_id}}/complete"), post(complete_session))
        .route(&format!("{BASE}/sessions/{{session_id}}/history"), get(get_session_history))
        .route(&format!("{BASE}/decide"), post(decide_route))
}

/// `action_client_scope`. `client_id` is the only tenant column these tables
/// have, and the header that fills it is caller-supplied — so a workspace token
/// gets a namespace derived from its own workspace and never gets to name
/// another tenant's. The master key keeps the header behaviour, so a
/// single-tenant deployment can still partition by hand.
fn client_scope(principal: &Principal, headers: &HeaderMap) -> Option<String> {
    match principal.workspace_id {
        Some(workspace_id) => Some(format!("ws:{workspace_id}")),
        None => crate::processes::client_header(headers),
    }
}

/// `_check_client_access`. An unowned row is public; an owned one needs the
/// caller to name the same namespace.
fn client_access(row_client_id: Option<&str>, scope: Option<&str>) -> bool {
    match row_client_id.filter(|id| !id.is_empty()) {
        None => true,
        Some(owner) => scope.is_some_and(|scope| owner == scope.trim()),
    }
}

fn access_denied() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "Access denied")
}

#[derive(FromRow)]
struct ActionSetRow {
    id: i64,
    client_id: Option<String>,
    name: String,
    description: Option<String>,
    metadata_json: Option<String>,
}

/// Every action column, for the CRUD routes — the engine's [`ActionRow`] carries
/// only the four the prompt needs.
#[derive(FromRow)]
struct ActionFullRow {
    id: i64,
    action_id: String,
    name: String,
    description: String,
    parameters_json: String,
    execution_mode: String,
    endpoint: Option<String>,
}

pub const ACTION_COLUMNS: &str = "CAST(id AS BIGINT) AS id, action_id, name, description, \
     parameters_json, execution_mode, endpoint";

impl ActionFullRow {
    /// `registry.action_to_dict`, which is also exactly `ActionResponse`.
    fn to_out(&self) -> Value {
        json!({
            "id": self.id,
            "action_id": self.action_id,
            "name": self.name,
            "description": self.description,
            "parameters": decode_object(&self.parameters_json),
            "execution_mode": self.execution_mode,
            "endpoint": self.endpoint,
        })
    }
}

/// `json.loads` behind a `try/except JSONDecodeError` returning `{}` — and a
/// stored scalar (`5`, `null`) is returned *as it is* by Python, since only a
/// decode failure is caught. That distinction is visible in a response body.
fn decode_object(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({}))
}

fn decode_object_opt(raw: Option<&str>) -> Value {
    match raw.filter(|s| !s.is_empty()) {
        None => json!({}),
        Some(raw) => decode_object(raw),
    }
}

async fn load_set(state: &AppState, set_id: i64) -> Result<ActionSetRow, ApiError> {
    sqlx::query_as(&crate::db::sql(
        "SELECT id, client_id, name, description, metadata_json FROM action_sets WHERE id = ?", state.backend)
    )
    .bind(set_id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Action set not found"))
}

async fn load_actions(state: &AppState, set_id: i64) -> Result<Vec<ActionFullRow>, ApiError> {
    Ok(sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {ACTION_COLUMNS} FROM actions WHERE set_id = ?"
    ), state.backend))
    .bind(set_id)
    .fetch_all(&state.any)
    .await?)
}

/// `registry.action_set_to_dict`, which `ActionSetResponse` then renders — so
/// `client_id` and the timestamps are dropped on the way out.
fn set_to_out(row: &ActionSetRow, actions: &[ActionFullRow]) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "description": row.description,
        "metadata": decode_object_opt(row.metadata_json.as_deref()),
        "actions": actions.iter().map(ActionFullRow::to_out).collect::<Vec<_>>(),
    })
}

// --- Action sets -----------------------------------------------------------

async fn create_set(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;

    let mut errors = Vec::new();
    let name = required_str(&mut errors, &body, "name");
    let description = optional_str(&mut errors, &body, "description");
    let metadata = object_field(&mut errors, &body, "metadata");
    let actions = parse_action_creates(&mut errors, body.get("actions"));
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let effective = crate::processes::merged_client_id(scope.as_deref(), None);
    if crate::processes::require_client_id_enabled() && effective.is_none() {
        return Err(ApiError::bad_request("client_id is required"));
    }

    let now = sql_now();
    let set_id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO action_sets (client_id, name, description, metadata_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(&effective)
    .bind(&name)
    .bind(&description)
    // `set_metadata`: an empty mapping is stored as NULL, not as `{}`.
    .bind(metadata.as_ref().filter(|m| !m.is_empty()).map(|m| Value::Object(m.clone()).to_string()))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    for action in &actions {
        insert_action(&state, set_id, action, &now).await?;
    }

    let row = load_set(&state, set_id).await?;
    let actions = load_actions(&state, set_id).await?;
    Ok(Json(set_to_out(&row, &actions)).into_response())
}

async fn list_sets(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let limit = int_query(query.as_deref(), "limit", 50)?;
    let effective = crate::processes::merged_client_id(scope.as_deref(), None);

    // `list_action_sets`: an unowned set is shared, so it is listed for every
    // caller rather than only for the one that owns nothing.
    // Both rewritten strings are bound to locals: a `match` arm's temporary
    // dies at the end of the arm, and the query borrows it past that.
    let owned_sql = crate::db::sql(
        "SELECT CAST(id AS BIGINT) AS id, client_id, name, description, metadata_json \
         FROM action_sets WHERE client_id = ? OR client_id IS NULL ORDER BY id DESC LIMIT ?",
        state.backend,
    )
    .into_owned();
    let all_sql = crate::db::sql(
        "SELECT CAST(id AS BIGINT) AS id, client_id, name, description, metadata_json \
         FROM action_sets ORDER BY id DESC LIMIT ?",
        state.backend,
    )
    .into_owned();
    let rows: Vec<ActionSetRow> = match &effective {
        Some(client_id) => sqlx::query_as(&owned_sql).bind(client_id).bind(limit),
        None => sqlx::query_as(&all_sql).bind(limit),
    }
    .fetch_all(&state.any)
    .await?;

    let mut out = Vec::new();
    for row in rows {
        if !client_access(row.client_id.as_deref(), scope.as_deref()) {
            continue;
        }
        let actions = load_actions(&state, row.id).await?;
        out.push(set_to_out(&row, &actions));
    }
    Ok(Json(json!({ "action_sets": out })).into_response())
}

/// The set, or the 404/403 pair every route in this domain opens with.
async fn require_set(
    state: &AppState,
    set_id: i64,
    scope: Option<&str>,
) -> Result<ActionSetRow, ApiError> {
    let row = load_set(state, set_id).await?;
    if !client_access(row.client_id.as_deref(), scope) {
        return Err(access_denied());
    }
    Ok(row)
}

async fn get_set(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(set_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let row = require_set(&state, set_id, scope.as_deref()).await?;
    let actions = load_actions(&state, set_id).await?;
    Ok(Json(set_to_out(&row, &actions)).into_response())
}

async fn update_set(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(set_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;

    let mut errors = Vec::new();
    let name = optional_str(&mut errors, &body, "name");
    let description = optional_str(&mut errors, &body, "description");
    let metadata = object_field_opt(&mut errors, &body, "metadata");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    require_set(&state, set_id, scope.as_deref()).await?;

    // `is not None` per field, so a key the caller omitted keeps its column —
    // and `updated_at` is not touched, because SQLModel has no `onupdate` here.
    if let Some(name) = name {
        sqlx::query(&crate::db::sql("UPDATE action_sets SET name = ? WHERE id = ?", state.backend))
            .bind(name)
            .bind(set_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(description) = description {
        sqlx::query(&crate::db::sql("UPDATE action_sets SET description = ? WHERE id = ?", state.backend))
            .bind(description)
            .bind(set_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(metadata) = metadata {
        sqlx::query(&crate::db::sql("UPDATE action_sets SET metadata_json = ? WHERE id = ?", state.backend))
            .bind(
                (!metadata.is_empty()).then(|| Value::Object(metadata).to_string()),
            )
            .bind(set_id)
            .execute(&state.any)
            .await?;
    }

    let row = load_set(&state, set_id).await?;
    let actions = load_actions(&state, set_id).await?;
    Ok(Json(set_to_out(&row, &actions)).into_response())
}

async fn delete_set(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(set_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    require_set(&state, set_id, scope.as_deref()).await?;
    sqlx::query(&crate::db::sql("DELETE FROM actions WHERE set_id = ?", state.backend))
        .bind(set_id)
        .execute(&state.any)
        .await?;
    sqlx::query(&crate::db::sql("DELETE FROM action_sets WHERE id = ?", state.backend))
        .bind(set_id)
        .execute(&state.any)
        .await?;
    Ok(Json(json!({ "success": true })).into_response())
}

// --- Actions ---------------------------------------------------------------

/// `ActionCreate`, which is also the element type of `ActionSetCreate.actions`.
struct ActionCreate {
    action_id: String,
    name: String,
    description: String,
    parameters: Option<Map<String, Value>>,
    execution_mode: String,
    endpoint: Option<String>,
}

async fn insert_action(
    state: &AppState,
    set_id: i64,
    action: &ActionCreate,
    now: &str,
) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO actions \
         (set_id, action_id, name, description, parameters_json, execution_mode, endpoint, \
          created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(set_id)
    .bind(&action.action_id)
    .bind(&action.name)
    .bind(&action.description)
    .bind(parameters_json(action.parameters.as_ref()))
    .bind(&action.execution_mode)
    .bind(&action.endpoint)
    .bind(now)
    .bind(now)
    .fetch_one(&state.any)
    .await?)
}

/// `set_parameters`: a falsy mapping is stored as the literal `"{}"`.
fn parameters_json(parameters: Option<&Map<String, Value>>) -> String {
    match parameters.filter(|p| !p.is_empty()) {
        Some(p) => Value::Object(p.clone()).to_string(),
        None => "{}".to_string(),
    }
}

async fn add_action(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(set_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let action = parse_action_create(&mut errors, &body, &[]);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    let action = action.expect("no errors means a body");

    require_set(&state, set_id, scope.as_deref()).await?;

    let duplicate: Option<i64> =
        sqlx::query_scalar(&crate::db::sql("SELECT id FROM actions WHERE set_id = ? AND action_id = ?", state.backend))
            .bind(set_id)
            .bind(&action.action_id)
            .fetch_optional(&state.any)
            .await?;
    if duplicate.is_some() {
        return Err(ApiError::bad_request(format!(
            "Action with action_id '{}' already exists in this set",
            action.action_id
        )));
    }

    let id = insert_action(&state, set_id, &action, &sql_now()).await?;
    let row = require_action_by_row_id(&state, id).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn require_action_by_row_id(state: &AppState, id: i64) -> Result<ActionFullRow, ApiError> {
    sqlx::query_as(&crate::db::sql(&format!("SELECT {ACTION_COLUMNS} FROM actions WHERE id = ?"), state.backend))
        .bind(id)
        .fetch_optional(&state.any)
        .await?
        .ok_or_else(|| ApiError::not_found("Action not found"))
}

async fn require_action(
    state: &AppState,
    set_id: i64,
    action_id: &str,
) -> Result<ActionFullRow, ApiError> {
    sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {ACTION_COLUMNS} FROM actions WHERE set_id = ? AND action_id = ?"
    ), state.backend))
    .bind(set_id)
    .bind(action_id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Action not found"))
}

async fn list_set_actions(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(set_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    require_set(&state, set_id, scope.as_deref()).await?;
    let actions = load_actions(&state, set_id).await?;
    Ok(Json(json!({
        "actions": actions.iter().map(ActionFullRow::to_out).collect::<Vec<_>>(),
    }))
    .into_response())
}

async fn get_action_detail(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId((set_id, action_id)): PathId<(i64, String)>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    require_set(&state, set_id, scope.as_deref()).await?;
    let action = require_action(&state, set_id, &action_id).await?;
    Ok(Json(action.to_out()).into_response())
}

async fn update_action_detail(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId((set_id, action_id)): PathId<(i64, String)>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let name = optional_str(&mut errors, &body, "name");
    let description = optional_str(&mut errors, &body, "description");
    let parameters = object_field_opt(&mut errors, &body, "parameters");
    let execution_mode = optional_str(&mut errors, &body, "execution_mode");
    let endpoint = optional_str(&mut errors, &body, "endpoint");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    require_set(&state, set_id, scope.as_deref()).await?;
    let action = require_action(&state, set_id, &action_id).await?;

    if let Some(name) = name {
        sqlx::query(&crate::db::sql("UPDATE actions SET name = ? WHERE id = ?", state.backend))
            .bind(name)
            .bind(action.id)
            .execute(&state.any)
            .await?;
    }
    if let Some(description) = description {
        sqlx::query(&crate::db::sql("UPDATE actions SET description = ? WHERE id = ?", state.backend))
            .bind(description)
            .bind(action.id)
            .execute(&state.any)
            .await?;
    }
    if let Some(parameters) = parameters {
        sqlx::query(&crate::db::sql("UPDATE actions SET parameters_json = ? WHERE id = ?", state.backend))
            .bind(parameters_json(Some(&parameters)))
            .bind(action.id)
            .execute(&state.any)
            .await?;
    }
    if let Some(execution_mode) = execution_mode {
        sqlx::query(&crate::db::sql("UPDATE actions SET execution_mode = ? WHERE id = ?", state.backend))
            .bind(execution_mode)
            .bind(action.id)
            .execute(&state.any)
            .await?;
    }
    if let Some(endpoint) = endpoint {
        sqlx::query(&crate::db::sql("UPDATE actions SET endpoint = ? WHERE id = ?", state.backend))
            .bind(endpoint)
            .bind(action.id)
            .execute(&state.any)
            .await?;
    }

    let row = require_action_by_row_id(&state, action.id).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn delete_action_endpoint(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId((set_id, action_id)): PathId<(i64, String)>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    require_set(&state, set_id, scope.as_deref()).await?;
    let action = require_action(&state, set_id, &action_id).await?;
    sqlx::query(&crate::db::sql("DELETE FROM actions WHERE id = ?", state.backend))
        .bind(action.id)
        .execute(&state.any)
        .await?;
    Ok(Json(json!({ "success": true })).into_response())
}

// --- Sessions --------------------------------------------------------------

#[derive(FromRow)]
struct SessionRow {
    id: i64,
    client_id: Option<String>,
    action_set_id: i64,
    goal: String,
    context_json: Option<String>,
    status: String,
    current_step: i64,
    max_steps: i64,
    execution_mode: String,
}

pub const SESSION_COLUMNS: &str = "CAST(id AS BIGINT) AS id, client_id, \
     CAST(action_set_id AS BIGINT) AS action_set_id, goal, context_json, status, \
     CAST(current_step AS BIGINT) AS current_step, \
     CAST(max_steps AS BIGINT) AS max_steps, execution_mode";

impl SessionRow {
    fn to_out(&self) -> Value {
        json!({
            "id": self.id,
            "action_set_id": self.action_set_id,
            "goal": self.goal,
            "context": decode_object_opt(self.context_json.as_deref()),
            "status": self.status,
            "current_step": self.current_step,
            "max_steps": self.max_steps,
            "execution_mode": self.execution_mode,
        })
    }

    fn context(&self) -> Map<String, Value> {
        match decode_object_opt(self.context_json.as_deref()) {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }
}

async fn require_session(
    state: &AppState,
    session_id: i64,
    scope: Option<&str>,
) -> Result<SessionRow, ApiError> {
    let row: SessionRow = sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {SESSION_COLUMNS} FROM action_sessions WHERE id = ?"
    ), state.backend))
    .bind(session_id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if !client_access(row.client_id.as_deref(), scope) {
        return Err(access_denied());
    }
    Ok(row)
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;

    let mut errors = Vec::new();
    let action_set_id = required_int(&mut errors, &body, "action_set_id");
    let goal = required_str(&mut errors, &body, "goal");
    let context = object_field(&mut errors, &body, "context");
    let execution_mode = defaulted_str(&mut errors, &body, "execution_mode", "client");
    let max_steps = bounded_int(&mut errors, &body, "max_steps", 10, 1, 50);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let effective = crate::processes::merged_client_id(scope.as_deref(), None);
    if crate::processes::require_client_id_enabled() && effective.is_none() {
        return Err(ApiError::bad_request("client_id is required"));
    }

    require_set(&state, action_set_id, scope.as_deref()).await?;
    if load_actions(&state, action_set_id).await?.is_empty() {
        return Err(ApiError::bad_request("Action set has no actions"));
    }

    let context = context.unwrap_or_default();
    let now = sql_now();
    let session_id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO action_sessions \
         (client_id, action_set_id, goal, context_json, status, current_step, max_steps, \
          execution_mode, created_at, updated_at, completed_at) \
         VALUES (?, ?, ?, ?, 'active', 0, ?, ?, ?, ?, NULL) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(&effective)
    .bind(action_set_id)
    .bind(&goal)
    .bind((!context.is_empty()).then(|| Value::Object(context).to_string()))
    .bind(max_steps)
    .bind(&execution_mode)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    let row = require_session(&state, session_id, scope.as_deref()).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(session_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let row = require_session(&state, session_id, scope.as_deref()).await?;
    Ok(Json(row.to_out()).into_response())
}

#[derive(FromRow)]
struct StepRow {
    step_number: i64,
    thought: Option<String>,
    actions_json: String,
    status: String,
}

/// `StepResponse`, whose `actions` go back through `PlannedAction` — so a stored
/// action that is missing a field or carries an extra one is *not* echoed
/// verbatim.
fn step_response(
    session_id: i64,
    step_number: i64,
    thought: Option<&str>,
    actions: &[PlannedAction],
    status: &str,
    execution_mode: &str,
    is_final: bool,
) -> Value {
    json!({
        "session_id": session_id,
        "step_number": step_number,
        "thought": thought,
        "actions": actions,
        "status": status,
        "execution_mode": execution_mode,
        "is_final": is_final,
    })
}

async fn request_step(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(session_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let step_context = object_field(&mut errors, &body, "context");
    // Parsed and discarded, exactly as Python does with it.
    let _require_confirmation = lax_bool(&mut errors, &body, "require_confirmation");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let session = require_session(&state, session_id, scope.as_deref()).await?;
    if session.status != "active" && session.status != "paused" {
        return Err(ApiError::bad_request(format!(
            "Session is {}, cannot request new steps",
            session.status
        )));
    }

    if session.current_step >= session.max_steps {
        complete(&state, session_id).await?;
        return Ok(Json(step_response(
            session_id,
            session.current_step,
            Some("Maximum steps reached"),
            &[],
            "completed",
            &session.execution_mode,
            true,
        ))
        .into_response());
    }

    let actions = list_actions(&state, session.action_set_id).await?;

    // History is one entry per *executed* action: the step's planned actions
    // joined to whichever result was submitted for the same step and id.
    let steps: Vec<StepRow> = sqlx::query_as(&crate::db::sql(
        "SELECT step_number, thought, actions_json, status FROM session_steps \
         WHERE session_id = ? ORDER BY step_number", state.backend)
    )
    .bind(session_id)
    .fetch_all(&state.any)
    .await?;
    let mut history = Vec::new();
    for step in &steps {
        for planned in decode_array(&step.actions_json) {
            let action_id = planned.get("action_id").cloned().unwrap_or(Value::Null);
            let result: Option<(Option<String>, Option<String>)> = sqlx::query_as(&crate::db::sql(
                "SELECT result_json, error FROM session_results \
                 WHERE session_id = ? AND step_number = ? AND action_id = ?", state.backend)
            )
            .bind(session_id)
            .bind(step.step_number)
            // `SessionResult.action_id` is a string column and the planned
            // action's id may be anything JSON allows; a non-string simply
            // matches nothing, which is what SQLAlchemy does with it too.
            .bind(action_id.as_str().unwrap_or_default())
            .fetch_optional(&state.any)
            .await?;
            if let Some((result_json, error)) = result {
                history.push(json!({
                    "action_id": action_id,
                    "result": result_json
                        .filter(|s| !s.is_empty())
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .unwrap_or(Value::Null),
                    "error": error,
                }));
            }
        }
    }

    let mut merged = session.context();
    merged.extend(step_context.unwrap_or_default());

    let (planned, thought, _usage) =
        decide_actions(&state, &session.goal, &merged, &actions, &history, "").await;

    if planned.is_empty() {
        complete(&state, session_id).await?;
        let thought = thought.filter(|t| !t.is_empty());
        return Ok(Json(step_response(
            session_id,
            session.current_step,
            Some(thought.as_deref().unwrap_or("No further actions needed")),
            &[],
            "completed",
            &session.execution_mode,
            true,
        ))
        .into_response());
    }

    let step_number = session.current_step + 1;
    sqlx::query(&crate::db::sql("UPDATE action_sessions SET current_step = ?, status = 'awaiting_execution' WHERE id = ?", state.backend))
        .bind(step_number)
        .bind(session_id)
        .execute(&state.any)
        .await?;
    sqlx::query(&crate::db::sql(
        "INSERT INTO session_steps \
         (session_id, step_number, thought, actions_json, status, created_at, executed_at) \
         VALUES (?, ?, ?, ?, 'pending', ?, NULL)", state.backend)
    )
    .bind(session_id)
    .bind(step_number)
    .bind(&thought)
    .bind(serde_json::to_string(&planned).unwrap_or_else(|_| "[]".into()))
    .bind(sql_now())
    .execute(&state.any)
    .await?;

    Ok(Json(step_response(
        session_id,
        step_number,
        thought.as_deref(),
        &planned,
        "awaiting_execution",
        &session.execution_mode,
        false,
    ))
    .into_response())
}

async fn complete(state: &AppState, session_id: i64) -> Result<(), ApiError> {
    sqlx::query(&crate::db::sql("UPDATE action_sessions SET status = 'completed', completed_at = ? WHERE id = ?", state.backend))
        .bind(sql_now())
        .bind(session_id)
        .execute(&state.any)
        .await?;
    Ok(())
}

fn decode_array(raw: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(items)) => items,
        // `get_actions` only catches a decode error, so a stored scalar raises
        // on iteration in Python — but no writer here can produce one.
        _ => Vec::new(),
    }
}

async fn submit_result(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(session_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let step_number = required_int(&mut errors, &body, "step_number");
    let action_id = required_str(&mut errors, &body, "action_id");
    let result = object_field(&mut errors, &body, "result");
    let error = optional_str(&mut errors, &body, "error");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let session = require_session(&state, session_id, scope.as_deref()).await?;

    let step: Option<i64> = sqlx::query_scalar(&crate::db::sql(
        "SELECT id FROM session_steps WHERE session_id = ? AND step_number = ?", state.backend)
    )
    .bind(session_id)
    .bind(step_number)
    .fetch_optional(&state.any)
    .await?;
    let step_id = step.ok_or_else(|| ApiError::not_found("Step not found"))?;

    let failed = error.as_ref().is_some_and(|e| !e.is_empty());
    let result = result.unwrap_or_default();
    sqlx::query(&crate::db::sql(
        "INSERT INTO session_results \
         (session_id, step_number, action_id, result_json, error, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)", state.backend)
    )
    .bind(session_id)
    .bind(step_number)
    .bind(&action_id)
    .bind((!result.is_empty()).then(|| Value::Object(result).to_string()))
    .bind(&error)
    .bind(sql_now())
    .execute(&state.any)
    .await?;

    sqlx::query(&crate::db::sql("UPDATE session_steps SET status = ?, executed_at = ? WHERE id = ?", state.backend))
        .bind(if failed { "failed" } else { "executed" })
        .bind(sql_now())
        .bind(step_id)
        .execute(&state.any)
        .await?;
    sqlx::query(&crate::db::sql("UPDATE action_sessions SET status = 'active' WHERE id = ?", state.backend))
        .bind(session_id)
        .execute(&state.any)
        .await?;

    Ok(Json(json!({
        "session_id": session_id,
        "step_number": step_number,
        "action_id": action_id,
        "status": if failed { "failed" } else { "success" },
        "next_step_available": session.current_step < session.max_steps,
    }))
    .into_response())
}

async fn complete_session(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(session_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    // The body is `CompleteSessionRequest | None`, so **no body at all** is
    // valid here — unlike every other route in this domain.
    let summary = if raw.is_empty() {
        None
    } else {
        let body = parse_body(&raw)?;
        let mut errors = Vec::new();
        let summary = optional_str(&mut errors, &body, "summary");
        if !errors.is_empty() {
            return Err(ApiError::validation(errors));
        }
        summary
    };

    require_session(&state, session_id, scope.as_deref()).await?;
    complete(&state, session_id).await?;
    Ok(Json(json!({ "session_id": session_id, "status": "completed", "summary": summary }))
        .into_response())
}

async fn get_session_history(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(session_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let session = require_session(&state, session_id, scope.as_deref()).await?;

    let steps: Vec<StepRow> = sqlx::query_as(&crate::db::sql(
        "SELECT step_number, thought, actions_json, status FROM session_steps \
         WHERE session_id = ? ORDER BY step_number", state.backend)
    )
    .bind(session_id)
    .fetch_all(&state.any)
    .await?;

    let mut steps_out = Vec::new();
    for step in &steps {
        // `PlannedAction(**a)` — a stored action that does not satisfy the model
        // raises a 500 there, which is not worth reproducing; an unparseable
        // entry is dropped instead, and only this server writes these rows.
        let planned: Vec<PlannedAction> = decode_array(&step.actions_json)
            .into_iter()
            .filter_map(|a| serde_json::from_value(a).ok())
            .collect();
        steps_out.push(step_response(
            session_id,
            step.step_number,
            step.thought.as_deref(),
            &planned,
            &step.status,
            &session.execution_mode,
            false,
        ));
    }

    let results: Vec<(i64, String, Option<String>)> = sqlx::query_as(&crate::db::sql(
        "SELECT step_number, action_id, error FROM session_results WHERE session_id = ?", state.backend)
    )
    .bind(session_id)
    .fetch_all(&state.any)
    .await?;

    let results_out: Vec<Value> = results
        .iter()
        .map(|(step_number, action_id, error)| {
            json!({
                "session_id": session_id,
                "step_number": step_number,
                "action_id": action_id,
                "status": if error.as_ref().is_some_and(|e| !e.is_empty()) { "failed" } else { "success" },
                "next_step_available": false,
            })
        })
        .collect();

    Ok(Json(json!({
        "session": session.to_out(),
        "steps": steps_out,
        "results": results_out,
    }))
    .into_response())
}

async fn decide_route(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let action_set_id = required_int(&mut errors, &body, "action_set_id");
    let goal = required_str(&mut errors, &body, "goal");
    let context = object_field(&mut errors, &body, "context");
    let execution_mode = defaulted_str(&mut errors, &body, "execution_mode", "client");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    require_set(&state, action_set_id, scope.as_deref()).await?;
    let actions = list_actions(&state, action_set_id).await?;
    if actions.is_empty() {
        return Err(ApiError::bad_request("Action set has no actions"));
    }

    let (planned, thought, _usage) = decide_actions(
        &state,
        &goal,
        &context.unwrap_or_default(),
        &actions,
        &[],
        "",
    )
    .await;

    Ok(Json(json!({
        "thought": thought,
        "actions": planned,
        "execution_mode": execution_mode,
    }))
    .into_response())
}

// --- Body fields -----------------------------------------------------------

fn required_int(errors: &mut Vec<Value>, body: &Value, field: &str) -> i64 {
    match body.get(field) {
        None => {
            errors.push(ApiError::field_error(field, "missing", "Field required"));
            0
        }
        Some(value) => crate::wire::lax_int_value(errors, field, value).unwrap_or_default(),
    }
}

/// `int` with `ge`/`le` and a default — one failure for the type, another for
/// the bound, never both.
fn bounded_int(
    errors: &mut Vec<Value>,
    body: &Value,
    field: &str,
    default: i64,
    min: i64,
    max: i64,
) -> i64 {
    let Some(value) = body.get(field) else { return default };
    let Some(parsed) = crate::wire::lax_int_value(errors, field, value) else { return default };
    if parsed < min {
        errors.push(ApiError::field_error(
            field,
            "greater_than_equal",
            &format!("Input should be greater than or equal to {min}"),
        ));
    } else if parsed > max {
        errors.push(ApiError::field_error(
            field,
            "less_than_equal",
            &format!("Input should be less than or equal to {max}"),
        ));
    }
    parsed
}

/// A `dict` field with `default_factory=dict`, so an absent key is `{}` and an
/// explicit `null` is a type failure.
fn object_field(errors: &mut Vec<Value>, body: &Value, field: &str) -> Option<Map<String, Value>> {
    match body.get(field) {
        None => Some(Map::new()),
        Some(Value::Object(map)) => Some(map.clone()),
        Some(_) => {
            errors.push(ApiError::field_error(
                field,
                "dict_type",
                "Input should be a valid dictionary",
            ));
            None
        }
    }
}

/// A `dict | None` field: absent and null both mean "leave it alone".
fn object_field_opt(
    errors: &mut Vec<Value>,
    body: &Value,
    field: &str,
) -> Option<Map<String, Value>> {
    match body.get(field) {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => Some(map.clone()),
        Some(_) => {
            errors.push(ApiError::field_error(
                field,
                "dict_type",
                "Input should be a valid dictionary",
            ));
            None
        }
    }
}

/// `?limit=` as FastAPI reads a plain `int` query parameter.
fn int_query(query: Option<&str>, name: &str, default: i64) -> Result<i64, ApiError> {
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key == name {
            return value.trim().parse::<i64>().map_err(|_| {
                ApiError::validation(vec![json!({
                    "type": "int_parsing",
                    "loc": ["query", name],
                    "msg": "Input should be a valid integer, unable to parse string as an integer",
                })])
            });
        }
    }
    Ok(default)
}

fn parse_action_creates(errors: &mut Vec<Value>, value: Option<&Value>) -> Vec<ActionCreate> {
    let Some(value) = value else { return Vec::new() };
    let Some(items) = value.as_array() else {
        errors.push(ApiError::field_error("actions", "list_type", "Input should be a valid list"));
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, item) in items.iter().enumerate() {
        // The index is an **integer** in `loc`, not a string.
        let prefix = vec![Value::from("actions"), Value::from(index)];
        if !item.is_object() {
            errors.push(json!({
                "type": "model_attributes_type",
                "loc": loc_with(&prefix, &[]),
                "msg": "Input should be a valid dictionary or object to extract fields from",
            }));
            continue;
        }
        if let Some(action) = parse_action_create(errors, item, &prefix) {
            out.push(action);
        }
    }
    out
}

fn loc_with(prefix: &[Value], tail: &[&str]) -> Vec<Value> {
    let mut loc = vec![Value::from("body")];
    loc.extend(prefix.iter().cloned());
    loc.extend(tail.iter().map(|s| Value::from(*s)));
    loc
}

/// One `ActionCreate`, nested under `prefix` when it came from a list.
fn parse_action_create(
    errors: &mut Vec<Value>,
    body: &Value,
    prefix: &[Value],
) -> Option<ActionCreate> {
    let before = errors.len();
    let mut string_field = |field: &str, required: bool| -> String {
        match body.get(field) {
            None => {
                if required {
                    errors.push(json!({
                        "type": "missing",
                        "loc": loc_with(prefix, &[field]),
                        "msg": "Field required",
                    }));
                }
                String::new()
            }
            Some(Value::String(s)) => s.clone(),
            Some(_) => {
                errors.push(json!({
                    "type": "string_type",
                    "loc": loc_with(prefix, &[field]),
                    "msg": "Input should be a valid string",
                }));
                String::new()
            }
        }
    };

    let action_id = string_field("action_id", true);
    let name = string_field("name", true);
    let description = string_field("description", true);
    let execution_mode = match body.get("execution_mode") {
        None => "client".to_string(),
        _ => string_field("execution_mode", false),
    };
    let endpoint = match body.get("endpoint") {
        None | Some(Value::Null) => None,
        _ => Some(string_field("endpoint", false)),
    };

    let parameters = match body.get("parameters") {
        None => Some(Map::new()),
        Some(Value::Object(map)) => Some(map.clone()),
        Some(_) => {
            errors.push(json!({
                "type": "dict_type",
                "loc": loc_with(prefix, &["parameters"]),
                "msg": "Input should be a valid dictionary",
            }));
            None
        }
    };

    if errors.len() != before {
        return None;
    }
    Some(ActionCreate {
        action_id,
        name,
        description,
        parameters,
        execution_mode,
        endpoint,
    })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(action_id: &str, name: &str, confidence: f64) -> PlannedAction {
        PlannedAction {
            action_id: action_id.to_string(),
            name: name.to_string(),
            parameters: Map::new(),
            confidence,
            reasoning: None,
        }
    }

    // --- fallback 1: one JSON object instead of a tool call -----------------

    #[test]
    fn json_object_reply_yields_thought_and_actions() {
        let decision = parse_decision_response(
            r#"{"reasoning": "  needs a subtask  ",
                "actions": [{"action_id": "create_item", "name": "Create", "parameters": {"title": "x"}, "confidence": 0.4}]}"#,
        )
        .unwrap();
        assert_eq!(decision.thought.as_deref(), Some("needs a subtask"));
        assert_eq!(decision.actions.len(), 1);
        assert_eq!(decision.actions[0].action_id, "create_item");
        assert_eq!(decision.actions[0].name, "Create");
        assert_eq!(decision.actions[0].parameters["title"], json!("x"));
        assert_eq!(decision.actions[0].confidence, 0.4);
    }

    #[test]
    fn json_reply_survives_a_fence_and_a_think_block() {
        let decision = parse_decision_response(
            "<think>hmm</think>\n```json\n{\"thought\": \"go\", \"actions\": []}\n```",
        )
        .unwrap();
        assert_eq!(decision.thought.as_deref(), Some("go"));
        assert!(decision.actions.is_empty());
    }

    #[test]
    fn json_action_without_a_name_reuses_its_id_and_defaults_confidence() {
        let decision =
            parse_decision_response(r#"{"actions": [{"action_id": " mark_done "}]}"#).unwrap();
        assert_eq!(decision.actions, vec![planned("mark_done", "mark_done", 0.9)]);
    }

    #[test]
    fn json_action_with_no_id_is_dropped_not_guessed() {
        let decision =
            parse_decision_response(r#"{"thought": "t", "actions": [{"name": "Create"}]}"#).unwrap();
        assert!(decision.actions.is_empty());
    }

    #[test]
    fn out_of_range_confidence_fails_the_step_the_way_pydantic_does() {
        let err = parse_decision_response(r#"{"actions": [{"action_id": "a", "confidence": 1.5}]}"#)
            .unwrap_err();
        assert!(err.contains("confidence"), "{err}");
    }

    #[test]
    fn an_empty_json_object_falls_through_to_the_text_parser() {
        // `{}` is neither a thought nor an action, so the tag parser runs — and
        // `looks_like_machine_output` then refuses to show the braces as prose.
        let decision = parse_decision_response("{}").unwrap();
        assert_eq!(decision, Decision::default());
    }

    // --- fallback 2: tags and prose ----------------------------------------

    #[test]
    fn reasoning_and_actions_tags_are_read() {
        let decision = parse_decision_response(
            "<reasoning> plan it </reasoning>\n<actions>[{\"action_id\": \"a\", \"name\": \"A\", \"confidence\": 0.5}]</actions>",
        )
        .unwrap();
        assert_eq!(decision.thought.as_deref(), Some("plan it"));
        assert_eq!(decision.actions, vec![planned("a", "A", 0.5)]);
    }

    #[test]
    fn thought_prefix_collects_until_an_action_line() {
        let decision =
            parse_decision_response("Thought: first\nsecond\nAction: mark_done\nthird").unwrap();
        assert_eq!(decision.thought.as_deref(), Some("first\nsecond"));
        assert_eq!(decision.actions, vec![planned("mark_done", "mark_done", 0.8)]);
    }

    #[test]
    fn a_bad_entry_in_an_actions_block_keeps_the_ones_before_it() {
        let decision = parse_decision_response(
            "<actions>[{\"action_id\": \"a\", \"name\": \"A\"}, {\"action_id\": 7}]</actions>",
        )
        .unwrap();
        assert_eq!(decision.actions, vec![planned("a", "A", 0.9)]);
    }

    #[test]
    fn an_unparseable_actions_block_is_swallowed() {
        let decision = parse_decision_response("<actions>not json</actions>").unwrap();
        assert!(decision.actions.is_empty());
    }

    // --- the bare-text parser ----------------------------------------------

    #[test]
    fn action_lines_collect_their_parameter_assignments() {
        let actions = parse_actions_from_text(
            "Here goes\n- Action: create_item extra words\n  title = \"Buy milk\"\n  count = 3\n\nAction: mark_done",
        )
        .unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].action_id, "create_item");
        assert_eq!(actions[0].confidence, 0.8);
        assert_eq!(actions[0].parameters["title"], json!("Buy milk"));
        assert_eq!(actions[0].parameters["count"], json!("3"));
        assert_eq!(actions[1].action_id, "mark_done");
        assert!(actions[1].parameters.is_empty());
    }

    #[test]
    fn a_parameter_line_before_any_action_is_ignored() {
        assert!(parse_actions_from_text("title = x\nnothing here").unwrap().is_empty());
    }

    #[test]
    fn an_action_line_with_no_id_fails_the_way_pythons_index_error_does() {
        assert!(parse_actions_from_text("Action:").is_err());
    }

    // --- the thought of last resort ----------------------------------------

    #[test]
    fn prose_becomes_the_thought_but_machine_output_never_does() {
        let prose = parse_decision_response("  Just do the next thing.  ").unwrap();
        assert_eq!(prose.thought.as_deref(), Some("Just do the next thing."));

        // A JSON fragment this parser could not read must not be shown to the
        // user as the assistant's own words.
        let broken = parse_decision_response("```json\n{\"actions\": [ truncated").unwrap();
        assert_eq!(broken.thought, None);

        let long = "x".repeat(500);
        assert_eq!(parse_decision_response(&long).unwrap().thought.unwrap().len(), 200);
    }

    // --- the pieces the fallbacks lean on -----------------------------------

    #[test]
    fn code_fences_and_think_blocks_come_off() {
        assert_eq!(strip_code_fences("```json\n{\"a\": 1}\n```"), "{\"a\": 1}");
        assert_eq!(strip_code_fences("```\n  x  \n```"), "x");
        assert_eq!(strip_code_fences("```{\"a\": 1}```"), "{\"a\": 1}");
        assert_eq!(strip_code_fences("<think>why</think>\n hi "), "hi");
        // Unterminated: neither rule fires, so the text survives whole.
        assert_eq!(strip_code_fences("<think>why\n```json"), "<think>why\n```json");
        assert_eq!(strip_code_fences("plain"), "plain");
    }

    #[test]
    fn context_renders_as_pythons_json_dumps() {
        assert_eq!(
            json_dumps_indent2(&json!({"a": [1, {"b": "Résumé"}], "c": {}})),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": \"R\\u00e9sum\\u00e9\"\n    }\n  ],\n  \"c\": {}\n}"
        );
    }

    #[test]
    fn the_user_message_carries_the_goal_then_the_context() {
        let context: Map<String, Value> = json!({"item": {"id": 1}}).as_object().unwrap().clone();
        assert_eq!(
            build_user_message("Plan it", &context, &[]),
            "Goal: Plan it\n\nContext: {\n  \"item\": {\n    \"id\": 1\n  }\n}"
        );
        assert_eq!(build_user_message("Plan it", &Map::new(), &[]), "Goal: Plan it");
    }

    #[test]
    fn conversation_history_is_appended_and_capped_at_twelve_turns() {
        let turns: Vec<Value> =
            (0..15).map(|i| json!({"role": "user", "content": format!("m{i}")})).collect();
        let context: Map<String, Value> =
            json!({"conversation_history": turns}).as_object().unwrap().clone();
        let message = build_user_message("g", &context, &[]);
        // The excluded key leaves an empty context object behind, which Python
        // still renders.
        assert!(message.contains("Context: {}"), "{message}");
        assert!(!message.contains("user: m2"), "{message}");
        assert!(message.contains("user: m3"), "{message}");
        assert!(message.contains("user: m14"), "{message}");
    }
}
