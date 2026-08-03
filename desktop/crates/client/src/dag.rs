//! Client-side planner-DAG validation, aligned with `app/dag_schema.validate_planner_dag`
//! (structure, unique ids, dependency refs, acyclicity). Ported from `web/src/api/dag.ts`.

use crate::types::{PlannerDag, SubagentNode};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

fn push_unique(errors: &mut Vec<String>, msg: String) {
    if !errors.contains(&msg) {
        errors.push(msg);
    }
}

/// Kahn topological order (same idea as `app/dag_schema._assert_acyclic`).
/// Only call on a DAG that already passed dependency reference checks.
pub fn planner_topological_uuids(dag: &PlannerDag) -> Vec<String> {
    let ids: Vec<&str> = dag.subagents.iter().map(|a| a.client_uuid.as_str()).collect();
    let id_set: HashSet<&str> = ids.iter().copied().collect();
    let mut indegree: HashMap<&str, usize> = ids.iter().map(|id| (*id, 0)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = ids.iter().map(|id| (*id, Vec::new())).collect();

    for a in &dag.subagents {
        for d in a.dependencies.as_deref().unwrap_or_default() {
            if !id_set.contains(d.as_str()) {
                continue;
            }
            *indegree.entry(a.client_uuid.as_str()).or_insert(0) += 1;
            adj.entry(d.as_str()).or_default().push(a.client_uuid.as_str());
        }
    }

    // Iterate `ids` (insertion order) rather than the hash set for determinism.
    let mut queue: VecDeque<&str> = ids
        .iter()
        .copied()
        .filter(|id| indegree.get(id).copied().unwrap_or(0) == 0)
        .collect();
    let mut out = Vec::new();
    while let Some(u) = queue.pop_front() {
        out.push(u.to_string());
        if let Some(children) = adj.get(u) {
            for v in children.clone() {
                let e = indegree.entry(v).or_insert(0);
                *e = e.saturating_sub(1);
                if *e == 0 {
                    queue.push_back(v);
                }
            }
        }
    }
    out
}

fn parse_subagent(raw: &Value, index: usize, errors: &mut Vec<String>) -> Option<SubagentNode> {
    let prefix = format!("subagents[{index}]");
    let Some(o) = raw.as_object() else {
        push_unique(errors, format!("{prefix} must be an object"));
        return None;
    };

    let get_str = |key: &str| o.get(key).and_then(Value::as_str);
    let client_uuid = get_str("client_uuid");
    if client_uuid.map(|s| s.trim().is_empty()).unwrap_or(true) {
        push_unique(errors, format!("{prefix}.client_uuid must be a non-empty string"));
    }
    for key in ["role", "system_prompt", "instructions"] {
        if get_str(key).is_none() {
            push_unique(errors, format!("{prefix}.{key} must be a string"));
        }
    }

    let mut dependencies: Option<Vec<String>> = None;
    let mut deps_bad = false;
    if let Some(d) = o.get("dependencies") {
        match d.as_array() {
            None => {
                push_unique(errors, format!("{prefix}.dependencies must be an array of strings"));
                deps_bad = true;
            }
            Some(arr) => {
                if arr.iter().any(|v| !v.is_string()) {
                    push_unique(errors, format!("{prefix}.dependencies must contain only strings"));
                    deps_bad = true;
                } else {
                    dependencies = Some(
                        arr.iter().map(|v| v.as_str().unwrap().to_string()).collect(),
                    );
                }
            }
        }
    }

    let mut model = None;
    if let Some(m) = o.get("model") {
        if !m.is_null() {
            match m.as_str() {
                Some(s) => model = Some(s.to_string()),
                None => push_unique(errors, format!("{prefix}.model must be a string or null")),
            }
        }
    }

    let mut bool_field = |key: &str| -> Option<bool> {
        match o.get(key) {
            None => None,
            Some(v) => match v.as_bool() {
                Some(b) => Some(b),
                None => {
                    push_unique(errors, format!("{prefix}.{key} must be a boolean"));
                    None
                }
            },
        }
    };
    let subdecompose = bool_field("subdecompose");
    let requires_review = bool_field("requires_review");

    let client_uuid = client_uuid?;
    if client_uuid.trim().is_empty()
        || get_str("role").is_none()
        || get_str("system_prompt").is_none()
        || get_str("instructions").is_none()
        || deps_bad
    {
        return None;
    }

    Some(SubagentNode {
        client_uuid: client_uuid.to_string(),
        role: get_str("role").unwrap().to_string(),
        system_prompt: get_str("system_prompt").unwrap().to_string(),
        instructions: get_str("instructions").unwrap().to_string(),
        dependencies,
        model,
        subdecompose,
        requires_review,
    })
}

/// Validate an already-parsed JSON value as a planner DAG.
pub fn validate_planner_dag(raw: &Value) -> Result<PlannerDag, Vec<String>> {
    let mut errors = Vec::new();
    let Some(o) = raw.as_object() else {
        return Err(vec!["Root value must be a JSON object.".to_string()]);
    };

    let team_name = o.get("team_name").and_then(Value::as_str);
    if team_name.is_none() {
        push_unique(&mut errors, "Field \"team_name\" must be a string.".to_string());
    }
    let goal_restatement = o.get("goal_restatement").and_then(Value::as_str);
    if goal_restatement.is_none() {
        push_unique(&mut errors, "Field \"goal_restatement\" must be a string.".to_string());
    }

    let sub_raw = o.get("subagents").and_then(Value::as_array);
    match sub_raw {
        None => push_unique(&mut errors, "Field \"subagents\" must be a non-empty array.".to_string()),
        Some(arr) if arr.is_empty() => push_unique(
            &mut errors,
            "Field \"subagents\" must contain at least one subagent.".to_string(),
        ),
        _ => {}
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut subagents = Vec::new();
    for (i, node_raw) in sub_raw.unwrap().iter().enumerate() {
        if let Some(node) = parse_subagent(node_raw, i, &mut errors) {
            subagents.push(node);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if subagents.is_empty() {
        return Err(vec!["No valid subagents could be parsed.".to_string()]);
    }

    let mut seen = HashSet::new();
    for a in &subagents {
        if !seen.insert(a.client_uuid.clone()) {
            push_unique(&mut errors, format!("Duplicate client_uuid: {:?}", a.client_uuid));
        }
    }

    let ids: HashSet<&str> = subagents.iter().map(|a| a.client_uuid.as_str()).collect();
    for a in &subagents {
        for d in a.dependencies.as_deref().unwrap_or_default() {
            if !ids.contains(d.as_str()) {
                push_unique(
                    &mut errors,
                    format!(
                        "Unknown dependency {:?} referenced by subagent {:?}",
                        d, a.client_uuid
                    ),
                );
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let dag = PlannerDag {
        team_name: team_name.unwrap().to_string(),
        goal_restatement: goal_restatement.unwrap().to_string(),
        subagents,
    };
    if planner_topological_uuids(&dag).len() != dag.subagents.len() {
        return Err(vec!["DAG contains a cycle (cyclic dependencies).".to_string()]);
    }
    Ok(dag)
}

/// Best-effort parse for UI (graph, board). Invalid JSON or schema yields None.
pub fn parse_planner_dag(json: Option<&str>) -> Option<PlannerDag> {
    let raw: Value = serde_json::from_str(json?).ok()?;
    validate_planner_dag(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(uuid: &str, deps: &[&str]) -> Value {
        json!({
            "client_uuid": uuid,
            "role": "r",
            "system_prompt": "s",
            "instructions": "i",
            "dependencies": deps,
        })
    }

    #[test]
    fn valid_dag_parses() {
        let raw = json!({
            "team_name": "t",
            "goal_restatement": "g",
            "subagents": [node("a", &[]), node("b", &["a"])],
        });
        let dag = validate_planner_dag(&raw).unwrap();
        assert_eq!(dag.subagents.len(), 2);
        assert_eq!(planner_topological_uuids(&dag), vec!["a", "b"]);
    }

    #[test]
    fn duplicate_uuid_rejected() {
        let raw = json!({
            "team_name": "t", "goal_restatement": "g",
            "subagents": [node("a", &[]), node("a", &[])],
        });
        let errs = validate_planner_dag(&raw).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("Duplicate client_uuid")), "{errs:?}");
    }

    #[test]
    fn unknown_dependency_rejected() {
        let raw = json!({
            "team_name": "t", "goal_restatement": "g",
            "subagents": [node("a", &["ghost"])],
        });
        let errs = validate_planner_dag(&raw).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("Unknown dependency")), "{errs:?}");
    }

    #[test]
    fn cycle_rejected() {
        let raw = json!({
            "team_name": "t", "goal_restatement": "g",
            "subagents": [node("a", &["b"]), node("b", &["a"])],
        });
        let errs = validate_planner_dag(&raw).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("cycle")), "{errs:?}");
    }

    #[test]
    fn missing_fields_rejected() {
        let raw = json!({
            "team_name": "t", "goal_restatement": "g",
            "subagents": [{"client_uuid": "a"}],
        });
        let errs = validate_planner_dag(&raw).unwrap_err();
        assert!(errs.iter().any(|e| e.contains(".role must be a string")), "{errs:?}");
    }

    #[test]
    fn empty_subagents_rejected() {
        let raw = json!({"team_name": "t", "goal_restatement": "g", "subagents": []});
        assert!(validate_planner_dag(&raw).is_err());
    }

    #[test]
    fn parse_helper_tolerates_garbage() {
        assert!(parse_planner_dag(None).is_none());
        assert!(parse_planner_dag(Some("not json")).is_none());
        assert!(parse_planner_dag(Some("{}")).is_none());
    }
}
