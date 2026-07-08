import type { PlannerDag, SubagentNode, TaskNodeRecord } from "../api/types";

export function taskMapByUuid(tasks: TaskNodeRecord[]): Map<string, TaskNodeRecord> {
  const m = new Map<string, TaskNodeRecord>();
  for (const t of tasks) {
    m.set(t.client_uuid, t);
  }
  return m;
}

/** Nesting depth from `parent_client_uuid` chains (0 = top-level). */
export function taskDepthByUuid(tasks: TaskNodeRecord[]): Map<string, number> {
  const byUuid = taskMapByUuid(tasks);
  const memo = new Map<string, number>();

  function depth(uuid: string): number {
    const hit = memo.get(uuid);
    if (hit !== undefined) return hit;
    const t = byUuid.get(uuid);
    const p = t?.parent_client_uuid?.trim();
    if (!p || !byUuid.has(p)) {
      memo.set(uuid, 0);
      return 0;
    }
    const d = 1 + depth(p);
    memo.set(uuid, d);
    return d;
  }

  for (const t of tasks) depth(t.client_uuid);
  return memo;
}

/** Execution waves: batch of subagents whose internal deps are satisfied in prior waves. */
export function computeWaves(dag: PlannerDag): string[][] {
  const sub = dag.subagents;
  const depMap = new Map(
    sub.map((s) => [s.client_uuid, new Set(s.dependencies ?? [])] as const),
  );
  const pending = new Set(sub.map((s) => s.client_uuid));
  const waves: string[][] = [];
  while (pending.size > 0) {
    const wave: string[] = [];
    for (const id of pending) {
      const deps = depMap.get(id)!;
      const internalDeps = [...deps].filter((d) => depMap.has(d));
      if (internalDeps.every((d) => !pending.has(d))) {
        wave.push(id);
      }
    }
    if (wave.length === 0) break;
    waves.push(wave);
    for (const id of wave) pending.delete(id);
  }
  return waves;
}

export function waveIndexByUuid(dag: PlannerDag): Map<string, number> {
  const waves = computeWaves(dag);
  const m = new Map<string, number>();
  waves.forEach((ids, i) => {
    for (const id of ids) m.set(id, i);
  });
  return m;
}

const BOARD_STATUSES = ["pending", "running", "awaiting_review", "completed", "failed"] as const;
export type BoardColumn = (typeof BOARD_STATUSES)[number];

export function normalizeTaskStatus(raw: string): BoardColumn {
  const s = raw.toLowerCase();
  if (
    s === "pending" ||
    s === "running" ||
    s === "awaiting_review" ||
    s === "completed" ||
    s === "failed"
  ) {
    return s;
  }
  return "pending";
}

export type BoardRow = {
  subagent: SubagentNode;
  task: TaskNodeRecord | null;
  column: BoardColumn;
};

export function parseTaskDependenciesJson(t: TaskNodeRecord): string[] {
  try {
    const d = JSON.parse(t.dependencies_json) as unknown;
    if (!Array.isArray(d)) return [];
    return d.filter((x): x is string => typeof x === "string");
  } catch {
    return [];
  }
}

/** True when every dependency UUID has a completed task row (for pixel / UI readiness). */
export function areTaskDependenciesSatisfied(task: TaskNodeRecord, allTasks: TaskNodeRecord[]): boolean {
  const deps = parseTaskDependenciesJson(task);
  if (deps.length === 0) return true;
  const completed = new Set(
    allTasks.filter((x) => x.status === "completed").map((x) => x.client_uuid),
  );
  return deps.every((d) => completed.has(d));
}

/** Cards for Kanban: one row per planner subagent; status from task row or pending. */
export function boardRowsFromDag(dag: PlannerDag | null, tasks: TaskNodeRecord[]): BoardRow[] {
  const map = taskMapByUuid(tasks);
  if (!dag) {
    return tasks.map((t) => ({
      subagent: taskToSyntheticSubagent(t),
      task: t,
      column: normalizeTaskStatus(t.status),
    }));
  }
  return dag.subagents.map((sub) => {
    const task = map.get(sub.client_uuid) ?? null;
    const column = task ? normalizeTaskStatus(task.status) : "pending";
    return { subagent: sub, task, column };
  });
}

type SnapshotRosterRole = { id: string; name: string };

function parseRosterRolesFromSnapshot(teamSnapshotJson: string | null | undefined): SnapshotRosterRole[] | null {
  if (!teamSnapshotJson?.trim()) return null;
  try {
    const data = JSON.parse(teamSnapshotJson) as { roster?: { roles?: unknown[] } };
    const roles = data.roster?.roles;
    if (!Array.isArray(roles) || roles.length === 0) return null;
    const out: SnapshotRosterRole[] = [];
    for (const r of roles) {
      if (!r || typeof r !== "object") continue;
      const o = r as { id?: unknown; name?: unknown };
      const id = typeof o.id === "string" ? o.id.trim() : "";
      const name = typeof o.name === "string" ? o.name.trim() : "";
      if (!id && !name) continue;
      out.push({ id: id || name, name: name || id });
    }
    return out.length ? out : null;
  } catch {
    return null;
  }
}

function plannerRoleMatchesRoster(plannerRole: string, rosterId: string, rosterName: string): boolean {
  const p = plannerRole.trim().toLowerCase();
  const id = rosterId.trim().toLowerCase();
  const name = rosterName.trim().toLowerCase();
  return p === id || p === name;
}

/**
 * Pixel strip / roster view: one tile per **team roster role** (stable order from snapshot),
 * matched to planner subagents by `role` ↔ roster id or name. Unmatched roles show as pending.
 * Without a snapshot, falls back to {@link boardRowsFromDag} (one row per subagent or task).
 */
export function boardRowsForPixelStrip(
  dag: PlannerDag | null,
  tasks: TaskNodeRecord[],
  teamSnapshotJson: string | null | undefined,
): BoardRow[] {
  const base = boardRowsFromDag(dag, tasks);
  const roster = parseRosterRolesFromSnapshot(teamSnapshotJson);
  if (!roster) return base;

  const usedUuids = new Set<string>();
  const out: BoardRow[] = [];

  for (const role of roster) {
    let found: BoardRow | undefined;
    for (const row of base) {
      if (usedUuids.has(row.subagent.client_uuid)) continue;
      if (plannerRoleMatchesRoster(row.subagent.role, role.id, role.name)) {
        found = row;
        usedUuids.add(row.subagent.client_uuid);
        break;
      }
    }
    if (found) {
      out.push(found);
    } else {
      const stableId = role.id.trim() || role.name.trim();
      out.push({
        subagent: {
          client_uuid: `roster:${stableId}`,
          role: role.name,
          system_prompt: "",
          instructions: "",
          dependencies: [],
        },
        task: null,
        column: "pending",
      });
    }
  }

  return out;
}

function taskToSyntheticSubagent(t: TaskNodeRecord): SubagentNode {
  return {
    client_uuid: t.client_uuid,
    role: t.role,
    system_prompt: t.system_prompt,
    instructions: t.instructions,
    model: t.llm_model,
  };
}

export interface TimelineRow {
  waveIndex: number;
  client_uuid: string;
  role: string;
  taskStatus: string;
}

export function buildTimelineRows(
  dag: PlannerDag | null,
  tasks: TaskNodeRecord[],
): TimelineRow[] {
  const map = taskMapByUuid(tasks);
  if (!dag) {
    return tasks.map((t) => ({
      waveIndex: 0,
      client_uuid: t.client_uuid,
      role: t.role,
      taskStatus: t.status,
    }));
  }
  const waves = computeWaves(dag);
  const rows: TimelineRow[] = [];
  waves.forEach((ids, waveIndex) => {
    for (const id of ids) {
      const sub = dag.subagents.find((s) => s.client_uuid === id);
      const task = map.get(id);
      rows.push({
        waveIndex,
        client_uuid: id,
        role: sub?.role ?? id,
        taskStatus: task?.status ?? "pending",
      });
    }
  });
  return rows;
}
