//! Validated planner DAG schema plus the acyclicity check. Port of
//! `app/dag_schema.py`.
//!
//! **The error strings are contract, not messages.** They land verbatim in the
//! 400 body of `POST /processes/{id}/approve` and `/retry`, so
//! `Duplicate client_uuid: 'x'`, `Unknown dependency 'd' referenced by subagent
//! 'a'` and `DAG contains a cycle (cyclic dependencies)` are reproduced
//! character for character — Python's `repr()` quoting included.
//!
//! `client/src/dag.rs` validates the same shape and is deliberately **not**
//! reused: it was ported from the deleted web app and carries that UI's wording,
//! and the server crate does not depend on the client crate.
//!
//! One deliberate approximation: `Invalid planner DAG: …` wraps a pydantic
//! `ValidationError`, whose text is `N validation error(s) for PlannerDag`, a
//! `loc` line, and an indented sentence carrying a `[type=…, input_value=…]`
//! envelope and a docs URL. The header, the `loc` line and the sentence are
//! reproduced; the envelope and the URL are not. Same call made for `assist`
//! (plan.md, step 3 note 3) — this is prompt/error text, nothing on the wire
//! that a client branches on.

use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Python-shaped JSON
// ---------------------------------------------------------------------------

/// `json.dumps` spacing, and optionally its `ensure_ascii` escaping.
///
/// Both halves are load-bearing here. `process.dag_json` and
/// `tasknode.dependencies_json` are stored and echoed back raw by
/// `GET /processes/{id}`, so `["a", "b"]` with the space is what a client sees;
/// and `apply_planner_success` / `merge_and_persist_subdag_expansion` dump with
/// the default `ensure_ascii=True` while `apply_validated_planner_to_process`
/// passes `ensure_ascii=False`, so the same DAG is stored two ways depending on
/// which path wrote it.
///
/// `workflow_engine::PythonJson` and `todos::EnsureAscii` were each half of
/// this, privately, in their own module — the same UTF-16 surrogate walk
/// written twice and the same separator pair written twice. They are gone;
/// this is the one renderer, and `compact` is the axis they differed on.
pub(crate) struct PyJson {
    pub ensure_ascii: bool,
    /// `separators=(",", ":")` rather than `json.dumps`'s default `(", ", ": ")`.
    /// serde_json's own compact form already *is* the tight pair, so this only
    /// has to stop widening it.
    pub compact: bool,
}

impl serde_json::ser::Formatter for PyJson {
    fn begin_array_value<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(if self.compact { b"," } else { b", " })
        }
    }

    fn begin_object_key<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(if self.compact { b"," } else { b", " })
        }
    }

    fn begin_object_value<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        writer.write_all(if self.compact { b":" } else { b": " })
    }

    fn write_string_fragment<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> std::io::Result<()> {
        if !self.ensure_ascii {
            return writer.write_all(fragment.as_bytes());
        }
        // Same walk as `todos::EnsureAscii`: DEL is ASCII but outside Python's
        // printable range, and an astral char comes out as a surrogate pair
        // because that is what `json.dumps` emits.
        let mut plain = 0;
        let mut units = [0u16; 2];
        for (i, ch) in fragment.char_indices() {
            if ch.is_ascii() && ch != '\u{7f}' {
                continue;
            }
            writer.write_all(fragment[plain..i].as_bytes())?;
            for unit in ch.encode_utf16(&mut units) {
                writer.write_all(format!("\\u{:04x}", *unit).as_bytes())?;
            }
            plain = i + ch.len_utf8();
        }
        writer.write_all(fragment[plain..].as_bytes())
    }
}

/// `json.dumps(value, ensure_ascii=…)` — default separators, no indent.
pub(crate) fn python_json<T: Serialize>(value: &T, ensure_ascii: bool) -> String {
    render(value, PyJson { ensure_ascii, compact: false })
}

/// `json.dumps(value, separators=(",", ":"), ensure_ascii=…)`.
///
/// The tight pair matters where the output is *stored* rather than echoed:
/// `process.team_snapshot_json` is written once and never re-derived, so its
/// bytes are the record.
pub(crate) fn python_json_compact<T: Serialize>(value: &T, ensure_ascii: bool) -> String {
    render(value, PyJson { ensure_ascii, compact: true })
}

fn render<T: Serialize>(value: &T, formatter: PyJson) -> String {
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    match value.serialize(&mut serializer) {
        Ok(()) => String::from_utf8(buffer).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// Python's `repr()` of a `str`, which is how every message below quotes a uuid.
///
/// Single quotes unless the value contains one and no double quote; backslash,
/// the quote itself and the three common escapes are escaped, other C0/C1
/// control characters become `\xNN`. Printable non-ASCII stays raw, as it does
/// in Python 3.
pub(crate) fn py_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(value.len() + 2);
    out.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

// ---------------------------------------------------------------------------
// Model alias sanitising
// ---------------------------------------------------------------------------

/// `dag_schema.sanitize_llm_model_alias`: `None` means "let the proxy pick".
///
/// Planners keep putting role slugs in `model` (`typescript-expert`), which is
/// not an alias the proxy can resolve, so those are dropped. Unicode hyphens
/// fold first — a model name copied out of prose arrives with an en dash.
///
/// The one copy: `todos.rs` (agent chat, agent step) and `workflows.rs` (assist)
/// call this rather than keeping their own, which is also what restores the
/// warning — Python logs it from every caller, and the two private copies did
/// not.
pub fn sanitize_llm_model_alias(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let folded: String = trimmed
        .chars()
        .map(|c| if ('\u{2010}'..='\u{2015}').contains(&c) { '-' } else { c })
        .collect();
    if is_role_slug(&folded.to_lowercase()) {
        logd!(
            "ignoring llm model {} \
             (looks like a role/skill slug, not a proxy model alias)",
            py_repr(&folded)
        );
        return None;
    }
    Some(folded)
}

/// `^[a-z][a-z0-9]{0,48}-(?:expert|scaffolder)$`, without a regex crate.
fn is_role_slug(lowered: &str) -> bool {
    let Some(head) =
        lowered.strip_suffix("-expert").or_else(|| lowered.strip_suffix("-scaffolder"))
    else {
        return false;
    };
    let mut chars = head.chars();
    let Some(first) = chars.next() else { return false };
    first.is_ascii_lowercase()
        && head.len() <= 49
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// The schema
// ---------------------------------------------------------------------------

/// Field order is pydantic's declaration order, because a derived `Serialize`
/// is the only thing that gives it — `serde_json::Map` sorts. That order is
/// what `planner_dag_to_json` writes into `process.dag_json`, which is a
/// cross-render target.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubagentSpec {
    pub client_uuid: String,
    pub role: String,
    pub system_prompt: String,
    pub instructions: String,
    pub dependencies: Vec<String>,
    /// JSON key `model`, matching OpenAI chat completions. The value must be a
    /// proxy alias (`GET /v1/models`), not a role or skill label.
    #[serde(rename = "model")]
    pub llm_model: Option<String>,
    pub subdecompose: bool,
    pub requires_review: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannerDag {
    pub team_name: String,
    pub goal_restatement: String,
    pub subagents: Vec<SubagentSpec>,
}

impl PlannerDag {
    pub fn spec(&self, client_uuid: &str) -> Option<&SubagentSpec> {
        self.subagents.iter().find(|s| s.client_uuid == client_uuid)
    }

    pub fn uuids(&self) -> Vec<String> {
        self.subagents.iter().map(|s| s.client_uuid.clone()).collect()
    }
}

/// `planner_dag_to_json_dict` + `json.dumps`. `ensure_ascii` is the caller's,
/// because the three writers of `dag_json` do not agree on it.
pub fn planner_dag_to_json(planner: &PlannerDag, ensure_ascii: bool) -> String {
    python_json(planner, ensure_ascii)
}

// ---------------------------------------------------------------------------
// Parsing (pydantic's half)
// ---------------------------------------------------------------------------

struct FieldErrors(Vec<(String, &'static str)>);

impl FieldErrors {
    fn push(&mut self, loc: impl Into<String>, msg: &'static str) {
        self.0.push((loc.into(), msg));
    }

    /// `str(ValidationError)`, minus the `[type=…]` envelope and the docs URL.
    fn render(&self) -> String {
        let n = self.0.len();
        let mut out = format!(
            "{n} validation error{} for PlannerDag",
            if n == 1 { "" } else { "s" }
        );
        for (loc, msg) in &self.0 {
            out.push('\n');
            out.push_str(loc);
            out.push_str("\n  ");
            out.push_str(msg);
        }
        out
    }
}

fn required_str(raw: &Value, key: &str, loc: &str, errors: &mut FieldErrors) -> String {
    match raw.get(key) {
        None | Some(Value::Null) => {
            errors.push(format!("{loc}{key}"), "Field required");
            String::new()
        }
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            errors.push(format!("{loc}{key}"), "Input should be a valid string");
            String::new()
        }
    }
}

/// Pydantic v2's lax bool coercion, which is what the planner's occasional
/// `"true"` relies on. Anything else is an error rather than a silent `false`.
fn coerce_bool(raw: Option<&Value>, loc: String, errors: &mut FieldErrors) -> bool {
    match raw {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => match n.as_f64() {
            Some(v) if v == 0.0 => false,
            Some(v) if v == 1.0 => true,
            _ => {
                errors.push(loc, "Input should be a valid boolean, unable to interpret input");
                false
            }
        },
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => false,
            "1" | "on" | "t" | "true" | "y" | "yes" => true,
            _ => {
                errors.push(loc, "Input should be a valid boolean, unable to interpret input");
                false
            }
        },
        Some(_) => {
            errors.push(loc, "Input should be a valid boolean, unable to interpret input");
            false
        }
    }
}

fn parse_subagent(raw: &Value, loc: &str, errors: &mut FieldErrors) -> SubagentSpec {
    if !raw.is_object() {
        errors.push(
            loc.trim_end_matches('.').to_string(),
            "Input should be a valid dictionary or instance of SubagentSpec",
        );
        return SubagentSpec {
            client_uuid: String::new(),
            role: String::new(),
            system_prompt: String::new(),
            instructions: String::new(),
            dependencies: Vec::new(),
            llm_model: None,
            subdecompose: false,
            requires_review: false,
        };
    }

    let client_uuid = required_str(raw, "client_uuid", loc, errors);
    let role = required_str(raw, "role", loc, errors);
    let system_prompt = required_str(raw, "system_prompt", loc, errors);
    let instructions = required_str(raw, "instructions", loc, errors);

    let dependencies = match raw.get("dependencies") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    _ => errors
                        .push(format!("{loc}dependencies.{i}"), "Input should be a valid string"),
                }
            }
            out
        }
        Some(_) => {
            errors.push(format!("{loc}dependencies"), "Input should be a valid list");
            Vec::new()
        }
    };

    // `field_validator("llm_model", mode="before")` runs before the type check,
    // so a non-string passes through untouched and then fails as a string.
    let llm_model = match raw.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => sanitize_llm_model_alias(s),
        Some(_) => {
            errors.push(format!("{loc}model"), "Input should be a valid string");
            None
        }
    };

    let subdecompose = coerce_bool(raw.get("subdecompose"), format!("{loc}subdecompose"), errors);
    let requires_review =
        coerce_bool(raw.get("requires_review"), format!("{loc}requires_review"), errors);

    SubagentSpec {
        client_uuid,
        role,
        system_prompt,
        instructions,
        dependencies,
        llm_model,
        subdecompose,
        requires_review,
    }
}

/// `SubagentSpec.model_validate(x)` on its own — what `merge_planner_with_new_subagents`
/// and the sub-DAG expansion loop call before the graph checks run.
pub fn validate_subagent(raw: &Value) -> Result<SubagentSpec, String> {
    let mut errors = FieldErrors(Vec::new());
    let spec = parse_subagent(raw, "", &mut errors);
    if errors.0.is_empty() {
        Ok(spec)
    } else {
        Err(errors.render().replacen("PlannerDag", "SubagentSpec", 1))
    }
}

fn parse_planner_dag(raw: &Value) -> Result<PlannerDag, String> {
    let mut errors = FieldErrors(Vec::new());
    if !raw.is_object() {
        return Err(
            "1 validation error for PlannerDag\n  \
             Input should be a valid dictionary or instance of PlannerDag"
                .to_string(),
        );
    }

    let team_name = required_str(raw, "team_name", "", &mut errors);
    let goal_restatement = required_str(raw, "goal_restatement", "", &mut errors);

    let subagents = match raw.get("subagents") {
        None | Some(Value::Null) => {
            errors.push("subagents", "Field required");
            Vec::new()
        }
        Some(Value::Array(items)) if items.is_empty() => {
            errors.push("subagents", "List should have at least 1 item after validation, not 0");
            Vec::new()
        }
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, item)| parse_subagent(item, &format!("subagents.{i}."), &mut errors))
            .collect(),
        Some(_) => {
            errors.push("subagents", "Input should be a valid list");
            Vec::new()
        }
    };

    if errors.0.is_empty() {
        Ok(PlannerDag { team_name, goal_restatement, subagents })
    } else {
        Err(errors.render())
    }
}

// ---------------------------------------------------------------------------
// Graph checks (the contract strings)
// ---------------------------------------------------------------------------

fn assert_unique_uuids(planner: &PlannerDag) -> Result<(), String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for a in &planner.subagents {
        if !seen.insert(a.client_uuid.as_str()) {
            return Err(format!("Duplicate client_uuid: {}", py_repr(&a.client_uuid)));
        }
    }
    Ok(())
}

fn assert_dependency_refs(planner: &PlannerDag) -> Result<(), String> {
    let ids: std::collections::HashSet<&str> =
        planner.subagents.iter().map(|a| a.client_uuid.as_str()).collect();
    for a in &planner.subagents {
        for d in &a.dependencies {
            if !ids.contains(d.as_str()) {
                return Err(format!(
                    "Unknown dependency {} referenced by subagent {}",
                    py_repr(d),
                    py_repr(&a.client_uuid)
                ));
            }
        }
    }
    Ok(())
}

/// Kahn topological sort; if not every node is processed, the graph has a cycle.
fn assert_acyclic(planner: &PlannerDag) -> Result<(), String> {
    use std::collections::HashMap;

    let mut indegree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for a in &planner.subagents {
        indegree.entry(a.client_uuid.as_str()).or_insert(0);
        adj.entry(a.client_uuid.as_str()).or_default();
    }
    for a in &planner.subagents {
        for d in &a.dependencies {
            adj.entry(d.as_str()).or_default().push(a.client_uuid.as_str());
            *indegree.entry(a.client_uuid.as_str()).or_insert(0) += 1;
        }
    }

    let mut queue: std::collections::VecDeque<&str> = indegree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut processed = 0usize;
    while let Some(u) = queue.pop_front() {
        processed += 1;
        for v in adj.get(u).into_iter().flatten() {
            let deg = indegree.get_mut(*v).expect("node registered above");
            *deg -= 1;
            if *deg == 0 {
                queue.push_back(v);
            }
        }
    }

    if processed != indegree.len() {
        return Err("DAG contains a cycle (cyclic dependencies)".to_string());
    }
    Ok(())
}

/// Every graph rule, in Python's order. Split out so `merge_planner_with_new_subagents`
/// can re-run them without re-parsing an already-parsed DAG.
pub fn validate_dag_graph(planner: &PlannerDag) -> Result<(), String> {
    assert_unique_uuids(planner)?;
    assert_dependency_refs(planner)?;
    assert_acyclic(planner)
}

/// Parse and validate planner output. The `Err` is the human-readable message
/// Python raises as a `ValueError` and the routes put in a 400 body.
pub fn validate_planner_dag(raw: &Value) -> Result<PlannerDag, String> {
    let planner = parse_planner_dag(raw).map_err(|e| format!("Invalid planner DAG: {e}"))?;
    validate_dag_graph(&planner)?;
    Ok(planner)
}

/// `validate_planner_dag(json.loads(s))`, with Python's JSON error wrapped the
/// way `process_approval_service.validate_approved_dag_json` wraps it.
pub fn validate_approved_dag_json(dag_json: &str) -> Result<PlannerDag, String> {
    let raw: Value = serde_json::from_str(dag_json)
        .map_err(|e| format!("Invalid JSON for approved DAG: {e}"))?;
    validate_planner_dag(&raw)
}

/// Append planner-validated subagents to an existing DAG and re-run the graph
/// checks. Python round-trips both halves through dicts and re-runs the whole
/// `validate_planner_dag`; `base` is already parsed and the caller has already
/// run each new spec through [`validate_subagent`], so only the graph half is
/// left to repeat.
pub fn merge_planner_with_new_subagents(
    base: &PlannerDag,
    new_subagents: &[SubagentSpec],
) -> Result<PlannerDag, String> {
    let mut subagents = base.subagents.clone();
    subagents.extend_from_slice(new_subagents);
    let merged = PlannerDag {
        team_name: base.team_name.clone(),
        goal_restatement: base.goal_restatement.clone(),
        subagents,
    };
    validate_dag_graph(&merged)?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent(uuid: &str, deps: &[&str]) -> Value {
        json!({
            "client_uuid": uuid,
            "role": "R",
            "system_prompt": "s",
            "instructions": "i",
            "dependencies": deps,
        })
    }

    fn dag(agents: Vec<Value>) -> Value {
        json!({ "team_name": "T", "goal_restatement": "G", "subagents": agents })
    }

    /// `test_dag_schema.py::test_valid_linear_chain`.
    #[test]
    fn a_linear_chain_validates() {
        let planner = validate_planner_dag(&dag(vec![agent("a", &[]), agent("b", &["a"])])).unwrap();
        assert_eq!(planner.subagents.len(), 2);
        assert_eq!(planner.team_name, "T");
        assert_eq!(planner.subagents[1].dependencies, vec!["a".to_string()]);
    }

    /// `test_duplicate_client_uuid`, `test_unknown_dependency`, `test_cycle_two_nodes`,
    /// `test_self_cycle`, `test_empty_subagents_rejected` — and the exact text,
    /// which is what the 400 body carries.
    #[test]
    fn every_graph_failure_keeps_pythons_wording() {
        assert_eq!(
            validate_planner_dag(&dag(vec![agent("x", &[]), agent("x", &[])])).unwrap_err(),
            "Duplicate client_uuid: 'x'"
        );
        assert_eq!(
            validate_planner_dag(&dag(vec![agent("a", &["missing"])])).unwrap_err(),
            "Unknown dependency 'missing' referenced by subagent 'a'"
        );
        assert_eq!(
            validate_planner_dag(&dag(vec![agent("a", &["b"]), agent("b", &["a"])])).unwrap_err(),
            "DAG contains a cycle (cyclic dependencies)"
        );
        // A self-edge is the one-node cycle.
        assert_eq!(
            validate_planner_dag(&dag(vec![agent("a", &["a"])])).unwrap_err(),
            "DAG contains a cycle (cyclic dependencies)"
        );
        // Duplicates are checked before dependency refs, so a DAG that breaks
        // both reports the duplicate.
        assert_eq!(
            validate_planner_dag(&dag(vec![agent("x", &["nope"]), agent("x", &[])])).unwrap_err(),
            "Duplicate client_uuid: 'x'"
        );
        // A partial cycle still fails even though the rest is orderable.
        assert!(validate_planner_dag(&dag(vec![
            agent("a", &[]),
            agent("b", &["c"]),
            agent("c", &["b"]),
        ]))
        .is_err());

        let err = validate_planner_dag(&dag(vec![])).unwrap_err();
        assert!(err.starts_with("Invalid planner DAG: "), "{err}");
        assert!(err.contains("List should have at least 1 item after validation, not 0"), "{err}");
    }

    #[test]
    fn a_missing_field_is_a_pydantic_style_error_naming_its_path() {
        let err = validate_planner_dag(&json!({
            "team_name": "T",
            "goal_restatement": "G",
            "subagents": [{"client_uuid": "a", "role": "R"}],
        }))
        .unwrap_err();
        assert!(err.starts_with("Invalid planner DAG: 2 validation errors for PlannerDag"), "{err}");
        assert!(err.contains("subagents.0.system_prompt\n  Field required"), "{err}");
        assert!(err.contains("subagents.0.instructions\n  Field required"), "{err}");
    }

    /// `test_optional_model_alias`, `test_planner_skill_slug_model_stripped`,
    /// the case-insensitive and unicode-hyphen variants, `react-scaffolder`, and
    /// `test_gemini_flash_model_kept`.
    #[test]
    fn role_slugs_are_stripped_from_model_and_real_aliases_are_not() {
        let with_model = |m: &str| {
            let mut a = agent("a", &[]);
            a["model"] = json!(m);
            validate_planner_dag(&dag(vec![a])).unwrap().subagents[0].llm_model.clone()
        };
        assert_eq!(with_model("local").as_deref(), Some("local"));
        assert_eq!(with_model("gemini-flash").as_deref(), Some("gemini-flash"));
        assert_eq!(with_model("typescript-expert"), None);
        assert_eq!(with_model("TypeScript-Expert"), None);
        assert_eq!(with_model("typescript\u{2011}expert"), None);
        assert_eq!(with_model("react-scaffolder"), None);
        // Blank and whitespace mean "server default", not an alias.
        assert_eq!(with_model("   "), None);
        // The unicode hyphen is folded even when the alias survives.
        assert_eq!(with_model("gemini\u{2013}flash").as_deref(), Some("gemini-flash"));
        // Two hyphens is not the slug shape.
        assert_eq!(with_model("a-b-expert").as_deref(), Some("a-b-expert"));
    }

    /// `test_dag_merge.py::test_merge_planner_with_new_subagents`.
    #[test]
    fn merging_appends_and_revalidates() {
        let base = validate_planner_dag(&dag(vec![agent("a", &[])])).unwrap();
        let new = |uuid: &str, deps: &[&str]| vec![validate_subagent(&agent(uuid, deps)).unwrap()];

        let merged = merge_planner_with_new_subagents(&base, &new("b", &["a"])).unwrap();
        assert_eq!(merged.subagents.len(), 2);
        assert_eq!(merged.subagents[1].client_uuid, "b");

        // A merge that reuses an id or points nowhere is refused with the same
        // strings the approve route answers with.
        assert_eq!(
            merge_planner_with_new_subagents(&base, &new("a", &["a"])).unwrap_err(),
            "Duplicate client_uuid: 'a'"
        );
        assert_eq!(
            merge_planner_with_new_subagents(&base, &new("b", &["ghost"])).unwrap_err(),
            "Unknown dependency 'ghost' referenced by subagent 'b'"
        );
    }

    #[test]
    fn a_single_subagent_validates_on_its_own() {
        let spec = validate_subagent(&agent("a", &["x"])).unwrap();
        assert_eq!(spec.client_uuid, "a");
        assert_eq!(spec.dependencies, vec!["x".to_string()]);
        // Unresolved dependencies are the graph's problem, not this one's.
        let err = validate_subagent(&json!({"client_uuid": "a"})).unwrap_err();
        assert!(err.contains("for SubagentSpec"), "{err}");
        assert!(err.contains("role\n  Field required"), "{err}");
    }

    #[test]
    fn the_canonical_dag_json_is_pythons_dump() {
        let planner = validate_planner_dag(&dag(vec![agent("a", &[]), agent("b", &["a"])])).unwrap();
        let json = planner_dag_to_json(&planner, false);
        // Declaration order, `model` not `llm_model`, and `json.dumps` spacing —
        // this string is stored on the process and echoed back by GET.
        assert_eq!(
            json,
            r#"{"team_name": "T", "goal_restatement": "G", "subagents": [{"client_uuid": "a", "role": "R", "system_prompt": "s", "instructions": "i", "dependencies": [], "model": null, "subdecompose": false, "requires_review": false}, {"client_uuid": "b", "role": "R", "system_prompt": "s", "instructions": "i", "dependencies": ["a"], "model": null, "subdecompose": false, "requires_review": false}]}"#
        );
    }

    #[test]
    fn ensure_ascii_matches_json_dumps_on_both_settings() {
        // `apply_planner_success` dumps with the default `ensure_ascii=True`,
        // `apply_validated_planner_to_process` passes `False` — same DAG, two
        // stored spellings, and both are read back raw by GET /processes/{id}.
        // An astral char is a surrogate pair, exactly as `json.dumps` writes it.
        let text = "\u{e9}\u{1f600}";
        assert_eq!(
            python_json(&json!({ "a": text }), true),
            "{\"a\": \"\\u00e9\\ud83d\\ude00\"}"
        );
        assert_eq!(python_json(&json!({ "a": text }), false), format!("{{\"a\": \"{text}\"}}"));
        // `tasknode.dependencies_json` is echoed raw by GET /processes/{id}.
        assert_eq!(python_json(&json!(["a", "b"]), true), r#"["a", "b"]"#);
        assert_eq!(python_json(&json!([]), true), "[]");
    }

    #[test]
    fn repr_quotes_the_way_python_does() {
        assert_eq!(py_repr("x"), "'x'");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("say \"hi\""), "'say \"hi\"'");
        assert_eq!(py_repr("both ' and \""), "'both \\' and \"'");
        assert_eq!(py_repr("a\nb"), "'a\\nb'");
        assert_eq!(py_repr("a\\b"), "'a\\\\b'");
    }

    #[test]
    fn a_planner_bool_written_as_a_string_still_gates_the_review() {
        let mut a = agent("a", &[]);
        a["requires_review"] = json!("true");
        a["subdecompose"] = json!(1);
        let planner = validate_planner_dag(&dag(vec![a])).unwrap();
        assert!(planner.subagents[0].requires_review);
        assert!(planner.subagents[0].subdecompose);

        let mut bad = agent("a", &[]);
        bad["requires_review"] = json!("maybe");
        assert!(validate_planner_dag(&dag(vec![bad])).unwrap_err().contains("valid boolean"));
    }

    #[test]
    fn unknown_keys_are_ignored_like_extra_ignore() {
        let mut a = agent("a", &[]);
        a["nonsense"] = json!({"deep": 1});
        let planner = validate_planner_dag(&dag(vec![a])).unwrap();
        assert_eq!(planner.subagents[0].client_uuid, "a");
    }

    #[test]
    fn approved_dag_json_reports_a_json_error_before_a_schema_one() {
        let err = validate_approved_dag_json("{").unwrap_err();
        assert!(err.starts_with("Invalid JSON for approved DAG: "), "{err}");
        assert!(validate_approved_dag_json(&dag(vec![agent("a", &[])]).to_string()).is_ok());
    }
}
