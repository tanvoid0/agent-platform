//! Port of `app/assistant/clarifying_form.py` — turning an `ask_clarifying_questions`
//! action into an interactive `PlanningFormSpec`-shaped form.
//!
//! Regexes, not hand-rolled parsing: the dozen small patterns here (paren-option
//! extraction, "prefer X or Y" splitting, field-kind inference) are genuinely
//! regex-shaped in the Python original, and re-deriving them by hand risked a
//! quieter divergence than pulling in the crate.
//!
//! They are compiled once. This is not a hot path — a handful of calls per chat
//! turn, in front of a model round-trip — so the win is not the microseconds:
//! it is that every pattern is now declared in one block where they can be read
//! against each other, and that a malformed one fails on first use rather than
//! on whichever call first reaches its branch.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

const MAX_FIELDS: usize = 12;
const MAX_OPTION_LEN: usize = 120;
const VALID_KINDS: &[&str] = &["boolean", "single_select", "multi_select", "text", "textarea"];

macro_rules! pattern {
    ($name:ident, $re:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($re).unwrap());
    };
}

// Anything not allowed in a generated field id.
pattern!(SLUG_STRIP, r"[^a-z0-9]+");
// The same for an id the model supplied, which may keep its case.
pattern!(ID_STRIP, r"[^a-zA-Z0-9_]");
// A trailing `(a, b, c)`, optionally followed by `?` — the option list.
pattern!(PAREN, r"\(([^)]+)\)\??\s*$");
pattern!(TRAILING_PAREN, r"\s*\([^)]+\)\s*$");
pattern!(EG_PREFIX, r"(?i)^e\.g\.?,?\s*");
pattern!(OPTION_SPLIT, r"(?i),|/|\s+or\s+");
pattern!(OR_SPLIT, r"(?i)\s+or\s+");
// Greedy `.*` on purpose; see `prefer_or_options`.
pattern!(PREFER, r"(?i)^.*\bprefer\b");
// Openers that make a question a yes/no one.
pattern!(AUX_VERB, r"(?i)^(do|does|did|is|can|will|should|would|have you|has)\b");
pattern!(ARE_THERE, r"^are there\b");
// `\b`, not `starts_with`: "whatever" does not open a question.
pattern!(WHAT, r"^what\b");
pattern!(COUNT_WORD, r"\b\d+\s*(days?|meals?|times?)\b");

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn strip_dots(s: &str) -> String {
    s.trim_matches('.').to_string()
}

const STRIP_PUNCT: &[char] = &[' ', '?', ':', '.', ','];

fn strip_punct(s: &str) -> String {
    s.trim_matches(STRIP_PUNCT).to_string()
}

/// `_slug_id`.
fn slug_id(index: usize, label: &str) -> String {
    let lowered = label.to_lowercase();
    let replaced = SLUG_STRIP.replace_all(&lowered, "_");
    let slug = truncate_chars(replaced.trim_matches('_'), 40);
    if slug.is_empty() {
        format!("q{index}")
    } else {
        slug
    }
}

/// `_parse_paren_options`: a trailing `(a, b, c)` or `(a or b)`, optionally
/// followed by `?`, becomes 2–8 option strings.
fn parse_paren_options(question: &str) -> Option<Vec<String>> {
    let caps = PAREN.captures(question.trim())?;
    let inner = caps.get(1)?.as_str().trim();

    let inner = EG_PREFIX.replace(inner, "").trim().to_string();
    if inner.is_empty() || inner.chars().count() > 200 {
        return None;
    }

    let opts: Vec<String> = OPTION_SPLIT
        .split(&inner)
        // First filter is on the *un*-dot-stripped trim (`if p.strip()`).
        .filter(|p| !p.trim().is_empty())
        .map(|p| strip_dots(p.trim()))
        // Second filter is on the dot-stripped value (`if o`), which can be
        // empty when the token was dots only (e.g. an ellipsis).
        .filter(|o| !o.is_empty())
        .map(|o| truncate_chars(&o, MAX_OPTION_LEN))
        .collect();

    if (2..=8).contains(&opts.len()) {
        Some(opts)
    } else {
        None
    }
}

/// `_prefer_or_options`: "Do you prefer X or Y?" → `["X", "Y"]`.
fn prefer_or_options(question: &str) -> Option<Vec<String>> {
    let lower = question.to_lowercase();
    if !lower.contains("prefer") || !lower.contains(" or ") {
        return None;
    }
    let parts: Vec<&str> = OR_SPLIT.splitn(question, 2).collect();
    if parts.len() != 2 {
        return None;
    }
    // Greedy `.*` anchored at the start: the *last* `prefer` in `parts[0]` is
    // where the cut lands, same as Python's backtracking engine settles on.
    let left = strip_punct(&PREFER.replace(parts[0], ""));
    let right = strip_punct(parts[1]);
    let opts: Vec<String> =
        [left, right].into_iter().filter(|o| !o.is_empty() && o.chars().count() < 120).collect();
    if opts.len() == 2 {
        Some(opts)
    } else {
        None
    }
}

/// `_infer_field_kind`.
fn infer_field_kind(question: &str, options: Option<&[String]>) -> &'static str {
    if let Some(options) = options {
        if !options.is_empty() {
            let lower = question.to_lowercase();
            const MULTI_HINTS: &[&str] =
                &["any of", "which of", "select all", "avoid", "restrictions", "allerg"];
            if MULTI_HINTS.iter().any(|hint| lower.contains(hint)) {
                return "multi_select";
            }
            return "single_select";
        }
    }
    let lower = question.to_lowercase();
    let lower = lower.trim();
    if ARE_THERE.is_match(lower) || WHAT.is_match(lower) {
        return "textarea";
    }
    if AUX_VERB.is_match(lower) && !lower.contains(" or ") && !lower.contains("how many") {
        return "boolean";
    }
    if lower.contains("how many") || COUNT_WORD.is_match(lower) {
        return "text";
    }
    if question.chars().count() > 100 {
        return "textarea";
    }
    "text"
}

fn is_missing_value(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    }
}

/// `_coerce_llm_field`.
fn coerce_llm_field(raw: &Map<String, Value>, index: usize) -> Option<Map<String, Value>> {
    let label = raw
        .get("label")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| raw.get("question").and_then(Value::as_str).filter(|s| !s.trim().is_empty()))?;
    let label = truncate_chars(label.trim(), 200);

    let fid = match raw.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(fid) => {
            truncate_chars(&ID_STRIP.replace_all(fid, "_"), 40)
        }
        None => slug_id(index, &label),
    };

    let kind = raw
        .get("kind")
        .and_then(Value::as_str)
        .filter(|k| VALID_KINDS.contains(k))
        .map(str::to_string)
        .unwrap_or_else(|| infer_field_kind(&label, None).to_string());

    // Key order is `id, label, kind, required, [helpText], [options]` — the
    // `kind` slot is filled here so a later fallback to `"text"` (below)
    // updates this entry in place rather than appending a duplicate at the
    // end, the way `serde_json::Map::insert` on an existing key always does.
    let mut field = Map::new();
    field.insert("id".into(), Value::String(fid));
    field.insert("label".into(), Value::String(label));
    field.insert("kind".into(), Value::String(kind.clone()));
    field.insert("required".into(), Value::Bool(raw.get("required") == Some(&Value::Bool(true))));
    if let Some(help) =
        raw.get("helpText").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
    {
        field.insert("helpText".into(), Value::String(truncate_chars(help, 400)));
    }

    if kind == "single_select" || kind == "multi_select" {
        let listed: Option<Vec<String>> = raw.get("options").and_then(Value::as_array).and_then(|opts| {
            if opts.len() < 2 {
                return None;
            }
            let strs: Vec<String> = opts
                .iter()
                .filter(|o| !o.is_null())
                .map(|o| crate::todos::python_str(o).as_str().unwrap_or_default().to_string())
                .filter(|s| !s.trim().is_empty())
                .map(|s| truncate_chars(s.trim(), MAX_OPTION_LEN))
                .take(8)
                .collect();
            Some(strs)
        });
        match listed {
            Some(opts) => {
                field.insert(
                    "options".into(),
                    Value::Array(opts.into_iter().map(Value::String).collect()),
                );
            }
            None => match parse_paren_options(field.get("label").and_then(Value::as_str).unwrap_or("")) {
                Some(opts) => {
                    field.insert(
                        "options".into(),
                        Value::Array(opts.into_iter().map(Value::String).collect()),
                    );
                }
                None => {
                    field.insert("kind".into(), Value::String("text".to_string()));
                }
            },
        }
    }
    Some(field)
}

/// `_field_from_question`.
fn field_from_question(question: &str, index: usize, profile: Option<&Map<String, Value>>) -> Map<String, Value> {
    let q = question.trim();
    let stripped = TRAILING_PAREN.replace(q, "");
    let label = if stripped.trim().is_empty() { q.to_string() } else { stripped.trim().to_string() };

    let options = parse_paren_options(q).or_else(|| prefer_or_options(q));
    let kind = infer_field_kind(q, options.as_deref());
    let fid = slug_id(index, &label);

    let mut field = Map::new();
    field.insert("id".into(), Value::String(fid.clone()));
    field.insert("label".into(), Value::String(truncate_chars(&label, 200)));
    field.insert("kind".into(), Value::String(kind.to_string()));
    field.insert("required".into(), Value::Bool(false));
    if let Some(opts) = &options {
        if kind == "single_select" || kind == "multi_select" {
            field.insert(
                "options".into(),
                Value::Array(opts.iter().cloned().map(Value::String).collect()),
            );
        }
    }

    if let Some(profile) = profile {
        let mut val = profile.get(&fid).cloned();
        if val.is_none() {
            let label_lower = label.to_lowercase();
            for (key, pv) in profile {
                let key_lower = key.to_lowercase();
                if label_lower.contains(&key_lower) || key_lower.replace('_', " ") == label_lower
                    || label_lower.contains(&key_lower.replace('_', " "))
                {
                    val = Some(pv.clone());
                    break;
                }
            }
        }
        if let Some(v) = val {
            if !is_missing_value(Some(&v)) {
                field.insert("default".into(), v);
            }
        }
    }
    field
}

/// `build_clarifying_form`.
pub(crate) fn build_clarifying_form(
    questions: &[String],
    title: Option<&str>,
    llm_fields: Option<&[Value]>,
    profile: Option<&Map<String, Value>>,
) -> Option<Value> {
    let form_title = truncate_chars(title.unwrap_or("A few quick questions").trim(), 200);
    let description = "Pick the options that fit you, or type where needed.";

    if let Some(llm_fields) = llm_fields {
        if !llm_fields.is_empty() {
            let mut fields = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (i, raw) in llm_fields.iter().take(MAX_FIELDS).enumerate() {
                let Some(raw) = raw.as_object() else { continue };
                let Some(mut f) = coerce_llm_field(raw, i) else { continue };
                let id = f.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(profile) = profile {
                    if !f.contains_key("default") {
                        if let Some(v) = profile.get(&id) {
                            if !is_missing_value(Some(v)) {
                                f.insert("default".into(), v.clone());
                            }
                        }
                    }
                }
                fields.push(Value::Object(f));
            }
            if !fields.is_empty() {
                return Some(serde_json::json!({
                    "purpose": "clarifying",
                    "title": form_title,
                    "description": description,
                    "fields": fields,
                }));
            }
        }
    }

    let qs: Vec<&String> = questions.iter().filter(|q| !q.trim().is_empty()).collect();
    if qs.is_empty() {
        return None;
    }
    let fields: Vec<Value> = qs
        .into_iter()
        .take(MAX_FIELDS)
        .enumerate()
        .map(|(i, q)| Value::Object(field_from_question(q, i, profile)))
        .collect();
    Some(serde_json::json!({
        "purpose": "clarifying",
        "title": form_title,
        "description": description,
        "fields": fields,
    }))
}

/// `is_clarifying_form`.
pub(crate) fn is_clarifying_form(form: Option<&Value>) -> bool {
    form.and_then(Value::as_object).and_then(|f| f.get("purpose")).and_then(Value::as_str)
        == Some("clarifying")
}

/// `format_clarifying_answers_message`: the synthetic user turn sent after a
/// clarifying form is submitted.
pub(crate) fn format_clarifying_answers_message(
    form: &Value,
    answers: &serde_json::Map<String, Value>,
) -> String {
    let fields = form.get("fields").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut id_to_label: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for f in &fields {
        let Some(obj) = f.as_object() else { continue };
        let Some(id) = obj.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            continue;
        };
        let label = obj.get("label").and_then(Value::as_str).unwrap_or(id);
        id_to_label.insert(id.to_string(), label.to_string());
    }

    let mut lines = vec!["My answers to your questions:".to_string(), String::new()];
    for (key, v) in answers {
        let label = id_to_label.get(key).cloned().unwrap_or_else(|| key.replace('_', " "));
        let val = match v {
            Value::Array(items) if items.is_empty() => "(none)".to_string(),
            Value::Array(items) => items
                .iter()
                .map(|x| crate::todos::python_str(x).as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            Value::Bool(true) => "Yes".to_string(),
            Value::Bool(false) => "No".to_string(),
            other => {
                let s = crate::todos::python_str(other).as_str().unwrap_or_default().trim().to_string();
                if s.is_empty() {
                    "(skipped)".to_string()
                } else {
                    s
                }
            }
        };
        lines.push(format!("- {label}: {val}"));
    }
    lines.push(String::new());
    lines.push("Please continue planning using these answers.".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paren_options_need_two_to_eight_and_a_trailing_position() {
        assert_eq!(
            parse_paren_options("What's your diet style (vegan, vegetarian, omnivore)?"),
            Some(vec!["vegan".into(), "vegetarian".into(), "omnivore".into()])
        );
        assert_eq!(parse_paren_options("Do you have allergies (e.g. nuts, dairy)?"), Some(vec!["nuts".into(), "dairy".into()]));
        assert_eq!(parse_paren_options("Pick one (a)"), None, "only one option");
        assert_eq!(parse_paren_options("No parens here"), None);
    }

    #[test]
    fn prefer_or_options_splits_on_the_last_prefer() {
        assert_eq!(
            prefer_or_options("Do you prefer mornings or evenings?"),
            Some(vec!["mornings".into(), "evenings".into()])
        );
        // "preference" contains "prefer" as a bare substring — Python's guard is
        // `"prefer" not in lower`, not a word-boundary check, so this string
        // passes it too. `prefer` never matches inside "Mornings" (the left half
        // after the split), so the left side is untouched.
        assert_eq!(
            prefer_or_options("Mornings or evenings, no preference word"),
            Some(vec!["Mornings".into(), "evenings, no preference word".into()])
        );
        assert_eq!(prefer_or_options("Mornings and evenings, no or/prefer words"), None);
    }

    #[test]
    fn infer_field_kind_matches_pythons_heuristics() {
        assert_eq!(infer_field_kind("Do you have a gym membership?", None), "boolean");
        assert_eq!(infer_field_kind("What is your goal?", None), "textarea");
        assert_eq!(infer_field_kind("How many meals per day?", None), "text");
        assert_eq!(infer_field_kind("Are there any injuries?", None), "textarea");
        let opts = vec!["a".to_string(), "b".to_string()];
        assert_eq!(infer_field_kind("Which of these apply?", Some(&opts)), "multi_select");
        assert_eq!(infer_field_kind("Pick one", Some(&opts)), "single_select");
    }

    #[test]
    fn slug_id_falls_back_to_index_when_the_label_has_no_alnum() {
        assert_eq!(slug_id(3, "What's your name?"), "what_s_your_name");
        assert_eq!(slug_id(2, "???"), "q2");
    }

    #[test]
    fn build_clarifying_form_prefers_llm_fields_and_falls_back_to_questions() {
        let form = build_clarifying_form(
            &["What is your name?".to_string()],
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(form["purpose"], "clarifying");
        assert_eq!(form["fields"].as_array().unwrap().len(), 1);
        assert!(is_clarifying_form(Some(&form)));
        assert!(!is_clarifying_form(Some(&serde_json::json!({"purpose": "other"}))));

        assert_eq!(build_clarifying_form(&[], None, None, None), None);
    }

    /// Byte-for-byte against `python -c` output — this is the field-order bug
    /// this module already had once (`kind` landed last instead of 3rd because
    /// the fallback-to-`"text"` branch re-inserted it after `options`).
    #[test]
    fn coerce_llm_field_key_order_matches_python() {
        let raw: Map<String, Value> = serde_json::from_str(
            r#"{"label":"Pick a color","kind":"single_select","options":["red","green","blue"]}"#,
        )
        .unwrap();
        let f1 = coerce_llm_field(&raw, 0).unwrap();
        assert_eq!(
            serde_json::to_string(&f1).unwrap(),
            r#"{"id":"pick_a_color","label":"Pick a color","kind":"single_select","required":false,"options":["red","green","blue"]}"#
        );

        let raw: Map<String, Value> = serde_json::from_str(
            r#"{"label":"Pick a color (red or blue)","kind":"single_select"}"#,
        )
        .unwrap();
        let f2 = coerce_llm_field(&raw, 0).unwrap();
        assert_eq!(
            serde_json::to_string(&f2).unwrap(),
            r#"{"id":"pick_a_color_red_or_blue","label":"Pick a color (red or blue)","kind":"single_select","required":false,"options":["red","blue"]}"#
        );

        // No options, no parens to fall back on: `kind` downgrades to "text"
        // in place, at position 3 — not appended after `required`.
        let raw: Map<String, Value> =
            serde_json::from_str(r#"{"label":"Pick a color","kind":"single_select"}"#).unwrap();
        let f3 = coerce_llm_field(&raw, 0).unwrap();
        assert_eq!(
            serde_json::to_string(&f3).unwrap(),
            r#"{"id":"pick_a_color","label":"Pick a color","kind":"text","required":false}"#
        );
    }
}
