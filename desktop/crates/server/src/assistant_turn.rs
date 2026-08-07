//! Pure helpers from `app/assistant/services/assistant_chat.py` — pending-form
//! extraction, action normalization, and the reply-text templates a chat turn
//! renders. Split out of `assistant.rs` because none of this touches SQL; it
//! is the same "no DB, own file" reasoning as `chat_usage.rs` and
//! `context_budget.rs`.
//!
//! Every function here takes/returns [`PlannedAction`](crate::action_orchestrator::PlannedAction)
//! rather than a re-declared type, because `assistant.schemas.PlannedActionOut`
//! *is* `todos.schemas.PlannedActionOut` in Python — one import, shared by both
//! domains — and `PlannedAction`'s `#[derive(Serialize)]` field order (`action_id,
//! name, parameters, confidence, reasoning`) already matches pydantic's
//! `model_dump()`, so `serde_json::to_value` is safe to use on it directly.

use serde_json::{Map, Value};

use crate::action_orchestrator::{py_truthy, PlannedAction};
use crate::chat_usage::{merge_llm_usages, LlmStepUsageOut};
use crate::clarifying_form::build_clarifying_form;

// ---------------------------------------------------------------------------
// Pending forms
// ---------------------------------------------------------------------------

/// `_form_from_action_params`.
pub(crate) fn form_from_action_params(params: &Map<String, Value>) -> Option<Map<String, Value>> {
    let form = params.get("form")?.as_object()?;
    if !matches!(form.get("fields"), Some(Value::Array(f)) if !f.is_empty()) {
        return None;
    }
    let mut out = form.clone();
    let domain = params
        .get("domain")
        .filter(|v| py_truthy(v))
        .or_else(|| form.get("domain").filter(|v| py_truthy(v)));
    if let Some(domain) = domain {
        if !out.get("domain").is_some_and(py_truthy) {
            out.insert("domain".into(), domain.clone());
        }
    }
    Some(out)
}

/// `_extract_pending_form`.
pub(crate) fn extract_pending_form(actions: &[PlannedAction]) -> Option<Map<String, Value>> {
    for a in actions {
        if a.action_id == "present_planning_form" || a.action_id == "ask_clarifying_questions" {
            if let Some(form) = form_from_action_params(&a.parameters) {
                return Some(form);
            }
        }
    }
    None
}

pub(crate) fn pending_has_interactive_form(actions: &[PlannedAction]) -> bool {
    extract_pending_form(actions).is_some()
}

/// `_actions_without_forms`.
pub(crate) fn actions_without_forms(actions: &[PlannedAction]) -> Vec<PlannedAction> {
    actions
        .iter()
        .filter(|a| {
            if a.action_id == "present_planning_form" {
                return false;
            }
            if a.action_id == "ask_clarifying_questions" && form_from_action_params(&a.parameters).is_some() {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

const INFORMATIONAL_ACTION_IDS: &[&str] =
    &["ask_clarifying_questions", "suggest_next_steps", "propose_review"];

/// `_pending_requires_approval`.
pub(crate) fn pending_requires_approval(actions: &[PlannedAction]) -> bool {
    let task = actions_without_forms(actions);
    !task.is_empty() && task.iter().any(|a| !INFORMATIONAL_ACTION_IDS.contains(&a.action_id.as_str()))
}

/// `_pending_is_informational_only`.
pub(crate) fn pending_is_informational_only(actions: &[PlannedAction]) -> bool {
    let task = actions_without_forms(actions);
    !task.is_empty() && task.iter().all(|a| INFORMATIONAL_ACTION_IDS.contains(&a.action_id.as_str()))
}

// ---------------------------------------------------------------------------
// Conversation shaping
// ---------------------------------------------------------------------------

/// `_format_conversation_for_planner`.
pub(crate) fn format_conversation_for_planner(messages: &[Value]) -> Vec<Value> {
    const MAX_TURNS: usize = 12;
    let turns: Vec<Value> = messages
        .iter()
        .filter_map(|m| {
            let obj = m.as_object()?;
            let role = obj.get("role").and_then(Value::as_str)?;
            if role != "user" && role != "assistant" {
                return None;
            }
            let content = obj.get("content").filter(|v| py_truthy(v))?;
            let content = crate::todos::python_str(content);
            let content = content.as_str()?.trim();
            if content.is_empty() {
                return None;
            }
            Some(serde_json::json!({ "role": role, "content": content }))
        })
        .collect();
    let start = turns.len().saturating_sub(MAX_TURNS);
    turns[start..].to_vec()
}

/// `_is_form_save_continuation_message`: detects the synthetic "Saved …
/// profile:" turn `submit_planning_form`'s auto-continue sends.
fn is_form_save_continuation_message(message: &str) -> bool {
    let first = message.trim().split('\n').next().unwrap_or("").trim().to_lowercase();
    first.starts_with("saved ") && first.contains(" profile:")
}

/// `_strip_redundant_profile_saves`.
pub(crate) fn strip_redundant_profile_saves(planned: Vec<PlannedAction>, message: &str) -> Vec<PlannedAction> {
    if !is_form_save_continuation_message(message) {
        return planned;
    }
    planned.into_iter().filter(|a| a.action_id != "store_user_profile").collect()
}

// ---------------------------------------------------------------------------
// Display names and normalization
// ---------------------------------------------------------------------------

fn action_display_name(action_id: &str) -> Option<&'static str> {
    Some(match action_id {
        "ask_clarifying_questions" => "A few quick questions",
        "suggest_next_steps" => "Suggested next steps",
        "present_planning_form" => "Details needed",
        "store_user_profile" => "Save to your profile",
        "create_item" => "Add to your board",
        "propose_review" => "Progress review",
        _ => return None,
    })
}

/// Python's `str.title()`: the first alphabetic character of each run of
/// non-alphabetic characters is upper-cased, everything else lower-cased —
/// not just a split-on-space capitalize, so `"it's_ok"` titles to `"It'S Ok"`
/// exactly as Python's does (the apostrophe ends the word).
fn python_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_alpha = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

/// `_friendly_action_name`.
fn friendly_action_name(action_id: &str, name: Option<&str>) -> String {
    let n = name.unwrap_or("").trim();
    if n.is_empty() || n == action_id {
        return action_display_name(action_id)
            .map(str::to_string)
            .unwrap_or_else(|| python_title(&action_id.replace('_', " ")));
    }
    n.to_string()
}

/// `_questions_from_action`.
fn questions_from_action(action: &PlannedAction) -> Vec<String> {
    let Some(qs) = action.parameters.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    qs.iter()
        .filter(|q| !q.is_null())
        .map(|q| crate::todos::python_str(q).as_str().unwrap_or_default().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `_format_questions_in_message`.
fn format_questions_in_message(questions: &[String]) -> String {
    match questions {
        [] => String::new(),
        [only] => only.clone(),
        many => many.iter().enumerate().map(|(i, q)| format!("{}. {q}", i + 1)).collect::<Vec<_>>().join("\n"),
    }
}

/// `_normalize_planned_actions`: drops an `ask_clarifying_questions` with no
/// usable questions, and builds its interactive form; otherwise renames an
/// action to its friendly display name when Python's model left `name` blank
/// or equal to the raw `action_id`.
pub(crate) fn normalize_planned_actions(
    planned: Vec<PlannedAction>,
    profile: Option<&Map<String, Value>>,
) -> Vec<PlannedAction> {
    let mut out = Vec::with_capacity(planned.len());
    for a in planned {
        let name = friendly_action_name(&a.action_id, Some(a.name.as_str()));
        if a.action_id == "ask_clarifying_questions" {
            let questions = questions_from_action(&a);
            if questions.is_empty() {
                continue;
            }
            let mut params = a.parameters.clone();
            params.insert(
                "questions".into(),
                Value::Array(questions.iter().cloned().map(Value::String).collect()),
            );
            let llm_fields: Option<Vec<Value>> =
                params.get("fields").and_then(Value::as_array).cloned();
            if let Some(form) =
                build_clarifying_form(&questions, Some(&name), llm_fields.as_deref(), profile)
            {
                params.insert("form".into(), form);
            }
            out.push(PlannedAction {
                action_id: a.action_id,
                name,
                parameters: params,
                confidence: a.confidence,
                reasoning: a.reasoning,
            });
            continue;
        }
        if name != a.name {
            out.push(PlannedAction { name, ..a });
        } else {
            out.push(a);
        }
    }
    out
}

/// `_maybe_inject_domain_form`.
pub(crate) fn maybe_inject_domain_form(
    profile_ctx: &Map<String, Value>,
    mut planned: Vec<PlannedAction>,
) -> Vec<PlannedAction> {
    if planned.iter().any(|a| a.action_id == "present_planning_form") {
        return planned;
    }
    let gaps = profile_ctx.get("active_profile_gaps");
    let has_gaps = matches!(gaps, Some(Value::Array(a)) if !a.is_empty());
    let domain = profile_ctx.get("active_domain").and_then(Value::as_str);
    let (Some(domain), true) = (domain, has_gaps) else { return planned };
    let Some(spec) = crate::assistant::domain_form_spec(domain) else { return planned };

    let mut params = Map::new();
    params.insert("domain".into(), Value::String(domain.to_string()));
    params.insert("form".into(), spec);
    let mut with_form = vec![PlannedAction {
        action_id: "present_planning_form".to_string(),
        name: "Present planning form".to_string(),
        parameters: params,
        confidence: 1.0,
        reasoning: Some("Required profile fields missing — showing intake form.".to_string()),
    }];
    with_form.append(&mut planned);
    with_form
}

// ---------------------------------------------------------------------------
// Reply text
// ---------------------------------------------------------------------------

/// `_thought_is_user_facing`: orchestrator narration ("prepared N actions",
/// "…for your review") must not leak into chat copy.
pub(crate) fn thought_is_user_facing(thought: Option<&str>) -> bool {
    let Some(thought) = thought.map(str::trim).filter(|t| !t.is_empty()) else { return false };
    let lower = thought.to_lowercase();
    if lower.contains("prepared") && lower.contains("action") {
        return false;
    }
    if lower.contains("for your review") {
        return false;
    }
    true
}

/// `_assistant_reply_for_actions` — the templates a turn's chat bubble comes
/// from when the model proposed actions. Ported as the same if/elif chain,
/// not restructured, so a future diff against Python stays line-shaped.
pub(crate) fn assistant_reply_for_actions(task_actions: &[PlannedAction], thought: Option<&str>) -> String {
    if task_actions.is_empty() {
        return "Let me know if you'd like me to break this down into tasks.".to_string();
    }
    if task_actions.len() == 1 {
        let a = &task_actions[0];
        let p = &a.parameters;
        match a.action_id.as_str() {
            "ask_clarifying_questions" => {
                if form_from_action_params(p).is_some() {
                    return "I need a few details before I can put together your plan — \
                             use the form below (yes/no, picks, or short answers)."
                        .to_string();
                }
                let questions = questions_from_action(a);
                if !questions.is_empty() {
                    let block = format_questions_in_message(&questions);
                    return format!(
                        "I need a few details before I can put together your plan:\n\n\
                         {block}\n\n\
                         Reply in chat with whatever you know — even partial answers help."
                    );
                }
                return "I need a few more details before I can put together your plan — \
                         share anything relevant in your next message."
                    .to_string();
            }
            "suggest_next_steps" => {
                if let Some(g) = p.get("guidance").and_then(Value::as_str).map(str::trim).filter(|g| !g.is_empty())
                {
                    return g.to_string();
                }
                return "Here are some suggested next steps.".to_string();
            }
            "create_item" => {
                if let Some(title) =
                    p.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty())
                {
                    return format!(
                        "I can add \"{title}\" to your board — confirm below when it looks right."
                    );
                }
            }
            "create_habit" => {
                if let Some(title) =
                    p.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty())
                {
                    return format!("I can track \"{title}\" as a habit on your board.");
                }
            }
            "break_down_task" => {
                if matches!(p.get("steps"), Some(Value::Array(s)) if !s.is_empty()) {
                    return "I've broken this into steps — review them below and add to your board \
                             if you'd like."
                        .to_string();
                }
                if let Some(g) = p.get("guidance").and_then(Value::as_str).map(str::trim).filter(|g| !g.is_empty())
                {
                    return g.to_string();
                }
                return "Here's a step-by-step plan — review below and add what you'd like to your \
                         board."
                    .to_string();
            }
            "store_user_profile" => {
                return "Got it — I'll use that for planning. Tell me what you'd like to tackle next."
                    .to_string();
            }
            "propose_review" => {
                if let Some(r) = p.get("reason").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty())
                {
                    return r.to_string();
                }
            }
            _ => {}
        }
        if thought_is_user_facing(thought) {
            return thought.unwrap().trim().to_string();
        }
    }
    if task_actions.iter().all(|a| INFORMATIONAL_ACTION_IDS.contains(&a.action_id.as_str())) {
        return "A few things to look at below — reply in chat when you're ready.".to_string();
    }
    let creates = task_actions.iter().filter(|a| a.action_id == "create_item" || a.action_id == "create_habit").count();
    if creates == task_actions.len() {
        return format!(
            "I've lined up {creates} item{} for your board — confirm below to add them.",
            if creates != 1 { "s" } else { "" }
        );
    }
    let n = task_actions.len();
    format!("I have {n} suggestion{} for your board — take a look below.", if n != 1 { "s" } else { "" })
}

// ---------------------------------------------------------------------------
// Message construction
// ---------------------------------------------------------------------------

/// `_resolve_pending_proposal_in_messages`: the most recent pending assistant
/// proposal, mutated in place — mirrors Python's in-place dict mutation.
pub(crate) fn resolve_pending_proposal_in_messages(messages: &mut [Value], status: &str) {
    for m in messages.iter_mut().rev() {
        let Some(obj) = m.as_object_mut() else { continue };
        if obj.get("role").and_then(Value::as_str) == Some("assistant")
            && obj.get("proposal_status").and_then(Value::as_str) == Some("pending")
        {
            obj.insert("proposal_status".into(), Value::String(status.to_string()));
            return;
        }
    }
}

/// `_assistant_message_with_usage`. Field order — `role, content, [usage],
/// [proposed_actions, proposal_status]` — is what a stored thread row and
/// therefore every later read of it carries.
pub(crate) fn assistant_message_with_usage(
    content: &str,
    usage_steps: Vec<LlmStepUsageOut>,
    proposed_actions: Option<&[PlannedAction]>,
) -> Value {
    let mut msg = Map::new();
    msg.insert("role".into(), Value::String("assistant".into()));
    msg.insert("content".into(), Value::String(content.to_string()));
    let turn = merge_llm_usages(usage_steps);
    if turn.total_tokens != 0 || turn.cost_usd != 0.0 {
        msg.insert(
            "usage".into(),
            serde_json::json!({
                "prompt_tokens": turn.prompt_tokens,
                "completion_tokens": turn.completion_tokens,
                "total_tokens": turn.total_tokens,
                "cost_usd": turn.cost_usd,
            }),
        );
    }
    if let Some(actions) = proposed_actions.filter(|a| !a.is_empty()) {
        let dumped: Vec<Value> = actions.iter().map(|a| serde_json::to_value(a).expect("PlannedAction serializes")).collect();
        msg.insert("proposed_actions".into(), Value::Array(dumped));
        msg.insert("proposal_status".into(), Value::String("pending".into()));
    }
    Value::Object(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(action_id: &str, name: &str, params: Value) -> PlannedAction {
        PlannedAction {
            action_id: action_id.to_string(),
            name: name.to_string(),
            parameters: params.as_object().cloned().unwrap_or_default(),
            confidence: 1.0,
            reasoning: None,
        }
    }

    /// Cross-checked against `python -c` calling the real
    /// `_assistant_reply_for_actions`/`_friendly_action_name`/
    /// `_format_conversation_for_planner`.
    #[test]
    fn reply_text_matches_python() {
        let a1 = action("create_item", "Add to your board", serde_json::json!({"title": "Buy milk"}));
        assert_eq!(
            assistant_reply_for_actions(&[a1], None),
            "I can add \"Buy milk\" to your board — confirm below when it looks right."
        );

        let a2 = action(
            "ask_clarifying_questions",
            "x",
            serde_json::json!({"questions": ["What is your goal?", "How many days per week?"]}),
        );
        assert_eq!(
            assistant_reply_for_actions(&[a2], None),
            "I need a few details before I can put together your plan:\n\n\
             1. What is your goal?\n2. How many days per week?\n\n\
             Reply in chat with whatever you know — even partial answers help."
        );

        let a3 = action("create_item", "n", serde_json::json!({"title": "A"}));
        let a4 = action("create_habit", "n", serde_json::json!({"title": "B"}));
        assert_eq!(
            assistant_reply_for_actions(&[a3, a4], None),
            "I've lined up 2 items for your board — confirm below to add them."
        );
    }

    #[test]
    fn friendly_action_name_titles_like_python() {
        assert_eq!(friendly_action_name("some_weird_action", None), "Some Weird Action");
        assert_eq!(friendly_action_name("some_weird_action", Some("")), "Some Weird Action");
        // Python's `str.title()`: the apostrophe ends the word, so the `s`
        // after it capitalizes too.
        assert_eq!(friendly_action_name("it's_ok", None), "It'S Ok");
        assert_eq!(friendly_action_name("create_item", None), "Add to your board");
    }

    #[test]
    fn conversation_formatting_drops_non_chat_roles_and_empty_content() {
        let messages = vec![
            serde_json::json!({"role": "user", "content": "hi"}),
            serde_json::json!({"role": "system", "content": "ignored"}),
            serde_json::json!({"role": "assistant", "content": ""}),
            serde_json::json!({"role": "assistant", "content": "hello there"}),
        ];
        assert_eq!(
            format_conversation_for_planner(&messages),
            vec![
                serde_json::json!({"role": "user", "content": "hi"}),
                serde_json::json!({"role": "assistant", "content": "hello there"}),
            ]
        );
    }
}
