//! The part of `app/action_orchestrator/` that `todos agent/step` calls, and
//! nothing else.
//!
//! `registry.list_actions` (one `SELECT`) and `engine.decide_actions` are here;
//! the router's own 685 lines of `/action-sets`, `/sessions` and `/decide` stay
//! proxied to Python. Nothing in this file is a route.
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

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::chat_usage::{coerce_int, LlmStepUsageOut};
use crate::dag_schema::sanitize_llm_model_alias;
use crate::error::ApiError;
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
    Ok(sqlx::query_as(
        "SELECT action_id, name, description, parameters_json FROM actions WHERE set_id = ?",
    )
    .bind(set_id)
    .fetch_all(&state.pool)
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
fn build_user_message(goal: &str, context: &Map<String, Value>) -> String {
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
        json!({"role": "user", "content": build_user_message(goal, context)}),
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
fn py_display(value: &Value) -> String {
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
            build_user_message("Plan it", &context),
            "Goal: Plan it\n\nContext: {\n  \"item\": {\n    \"id\": 1\n  }\n}"
        );
        assert_eq!(build_user_message("Plan it", &Map::new()), "Goal: Plan it");
    }

    #[test]
    fn conversation_history_is_appended_and_capped_at_twelve_turns() {
        let turns: Vec<Value> =
            (0..15).map(|i| json!({"role": "user", "content": format!("m{i}")})).collect();
        let context: Map<String, Value> =
            json!({"conversation_history": turns}).as_object().unwrap().clone();
        let message = build_user_message("g", &context);
        // The excluded key leaves an empty context object behind, which Python
        // still renders.
        assert!(message.contains("Context: {}"), "{message}");
        assert!(!message.contains("user: m2"), "{message}");
        assert!(message.contains("user: m3"), "{message}");
        assert!(message.contains("user: m14"), "{message}");
    }
}
