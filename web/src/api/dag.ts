import type { PlannerDag, SubagentNode } from "./types";

export type PlannerDagParseResult =
  | { ok: true; dag: PlannerDag }
  | { ok: false; errors: string[] };

function pushUnique(errors: string[], msg: string) {
  if (!errors.includes(msg)) errors.push(msg);
}

function isString(v: unknown): v is string {
  return typeof v === "string";
}

/**
 * Kahn topological order (same idea as `app/dag_schema._assert_acyclic`).
 * Only call on a DAG that already passed dependency reference checks.
 */
export function plannerTopologicalUuids(dag: PlannerDag): string[] {
  const ids = dag.subagents.map((a) => a.client_uuid);
  const idSet = new Set(ids);
  const indegree = new Map<string, number>();
  const adj = new Map<string, string[]>();
  for (const id of ids) {
    indegree.set(id, 0);
    adj.set(id, []);
  }
  for (const a of dag.subagents) {
    for (const d of a.dependencies ?? []) {
      if (!idSet.has(d)) continue;
      indegree.set(a.client_uuid, (indegree.get(a.client_uuid) ?? 0) + 1);
      adj.get(d)!.push(a.client_uuid);
    }
  }
  const queue: string[] = [];
  for (const id of idSet) {
    if ((indegree.get(id) ?? 0) === 0) queue.push(id);
  }
  const out: string[] = [];
  while (queue.length) {
    const u = queue.shift()!;
    out.push(u);
    for (const v of adj.get(u) ?? []) {
      const next = (indegree.get(v) ?? 0) - 1;
      indegree.set(v, next);
      if (next === 0) queue.push(v);
    }
  }
  return out;
}

function parseSubagent(raw: unknown, index: number, errors: string[]): SubagentNode | null {
  const prefix = `subagents[${index}]`;
  if (!raw || typeof raw !== "object") {
    pushUnique(errors, `${prefix} must be an object`);
    return null;
  }
  const o = raw as Record<string, unknown>;
  const client_uuid = o.client_uuid;
  const role = o.role;
  const system_prompt = o.system_prompt;
  const instructions = o.instructions;
  if (!isString(client_uuid) || !client_uuid.trim()) {
    pushUnique(errors, `${prefix}.client_uuid must be a non-empty string`);
  }
  if (!isString(role)) pushUnique(errors, `${prefix}.role must be a string`);
  if (!isString(system_prompt)) pushUnique(errors, `${prefix}.system_prompt must be a string`);
  if (!isString(instructions)) pushUnique(errors, `${prefix}.instructions must be a string`);

  let dependencies: string[] | undefined;
  if (o.dependencies !== undefined) {
    if (!Array.isArray(o.dependencies)) {
      pushUnique(errors, `${prefix}.dependencies must be an array of strings`);
    } else {
      const bad = o.dependencies.find((d) => typeof d !== "string");
      if (bad !== undefined) {
        pushUnique(errors, `${prefix}.dependencies must contain only strings`);
      } else {
        dependencies = o.dependencies as string[];
      }
    }
  }

  let model: string | null | undefined;
  if (o.model !== undefined && o.model !== null) {
    if (!isString(o.model)) pushUnique(errors, `${prefix}.model must be a string or null`);
    else model = o.model;
  }

  let subdecompose: boolean | undefined;
  if (o.subdecompose !== undefined) {
    if (typeof o.subdecompose !== "boolean") {
      pushUnique(errors, `${prefix}.subdecompose must be a boolean`);
    } else subdecompose = o.subdecompose;
  }

  let requires_review: boolean | undefined;
  if (o.requires_review !== undefined) {
    if (typeof o.requires_review !== "boolean") {
      pushUnique(errors, `${prefix}.requires_review must be a boolean`);
    } else requires_review = o.requires_review;
  }

  if (
    !isString(client_uuid) ||
    !client_uuid.trim() ||
    !isString(role) ||
    !isString(system_prompt) ||
    !isString(instructions) ||
    (o.dependencies !== undefined && dependencies === undefined)
  ) {
    return null;
  }

  const node: SubagentNode = {
    client_uuid,
    role,
    system_prompt,
    instructions,
    dependencies,
    model,
    subdecompose,
    requires_review,
  };
  return node;
}

/**
 * Client-side validation aligned with `app/dag_schema.validate_planner_dag`
 * (structure, unique ids, dependency refs, acyclicity).
 */
export function validatePlannerDag(raw: unknown): PlannerDagParseResult {
  const errors: string[] = [];
  if (!raw || typeof raw !== "object") {
    return { ok: false, errors: ["Root value must be a JSON object."] };
  }
  const o = raw as Record<string, unknown>;

  if (!isString(o.team_name)) pushUnique(errors, 'Field "team_name" must be a string.');
  if (!isString(o.goal_restatement)) {
    pushUnique(errors, 'Field "goal_restatement" must be a string.');
  }

  const subRaw = o.subagents;
  if (!Array.isArray(subRaw)) {
    pushUnique(errors, 'Field "subagents" must be a non-empty array.');
  } else if (subRaw.length === 0) {
    pushUnique(errors, 'Field "subagents" must contain at least one subagent.');
  }

  if (errors.length) return { ok: false, errors };

  const subagents: SubagentNode[] = [];
  for (let i = 0; i < (subRaw as unknown[]).length; i++) {
    const node = parseSubagent((subRaw as unknown[])[i], i, errors);
    if (node) subagents.push(node);
  }

  if (errors.length) return { ok: false, errors };
  if (subagents.length === 0) {
    return { ok: false, errors: ["No valid subagents could be parsed."] };
  }

  const seen = new Set<string>();
  for (const a of subagents) {
    if (seen.has(a.client_uuid)) {
      pushUnique(errors, `Duplicate client_uuid: ${JSON.stringify(a.client_uuid)}`);
    }
    seen.add(a.client_uuid);
  }

  const ids = new Set(subagents.map((a) => a.client_uuid));
  for (const a of subagents) {
    for (const d of a.dependencies ?? []) {
      if (!ids.has(d)) {
        pushUnique(
          errors,
          `Unknown dependency ${JSON.stringify(d)} referenced by subagent ${JSON.stringify(a.client_uuid)}`,
        );
      }
    }
  }

  if (errors.length) return { ok: false, errors };

  const order = plannerTopologicalUuids({
    team_name: o.team_name as string,
    goal_restatement: o.goal_restatement as string,
    subagents,
  });
  if (order.length !== subagents.length) {
    pushUnique(errors, "DAG contains a cycle (cyclic dependencies).");
    return { ok: false, errors };
  }

  const dag: PlannerDag = {
    team_name: o.team_name as string,
    goal_restatement: o.goal_restatement as string,
    subagents,
  };
  return { ok: true, dag };
}

/**
 * Best-effort parse for UI (graph, board). Invalid JSON or schema yields null.
 */
export function parsePlannerDag(json: string | null): PlannerDag | null {
  if (!json) return null;
  try {
    const raw = JSON.parse(json) as unknown;
    const result = validatePlannerDag(raw);
    return result.ok ? result.dag : null;
  } catch {
    return null;
  }
}

export function shortUuid(uuid: string, head = 8): string {
  const t = uuid.trim();
  if (t.length <= head) return t;
  return `${t.slice(0, head)}…`;
}
