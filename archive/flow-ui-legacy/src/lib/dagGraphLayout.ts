import type { SubagentNode, TaskNodeRecord } from "../api/types";
import { taskDepthByUuid, taskMapByUuid } from "./dagTasks";

/** Horizontal spacing between nodes; vertical spacing between depth rows. */
const COL_W = 230;
const ROW_H = 130;

export type LineageVisibility = "all" | "depth_le_1" | "roots";

export function maxDepthForVisibility(v: LineageVisibility): number | null {
  if (v === "all") return null;
  if (v === "depth_le_1") return 1;
  return 0;
}

/** Subagents visible under the lineage cap (null maxDepth = no cap). */
export function visibleSubagentUuids(
  subagents: SubagentNode[],
  tasks: TaskNodeRecord[],
  maxDepth: number | null,
): Set<string> {
  const depthMap = taskDepthByUuid(tasks);
  const out = new Set<string>();
  for (const a of subagents) {
    const d = depthMap.get(a.client_uuid) ?? 0;
    if (maxDepth === null || d <= maxDepth) out.add(a.client_uuid);
  }
  return out;
}

/**
 * Layered grid: row = lineage depth, column = stable order within row (planner order).
 */
export function lineageLayoutPositions(
  subagents: SubagentNode[],
  tasks: TaskNodeRecord[],
  visible: Set<string>,
): Map<string, { x: number; y: number }> {
  const depthMap = taskDepthByUuid(tasks);
  const orderIndex = new Map(subagents.map((s, i) => [s.client_uuid, i]));

  const byDepth = new Map<number, string[]>();
  for (const a of subagents) {
    const id = a.client_uuid;
    if (!visible.has(id)) continue;
    const d = depthMap.get(id) ?? 0;
    if (!byDepth.has(d)) byDepth.set(d, []);
    byDepth.get(d)!.push(id);
  }
  for (const ids of byDepth.values()) {
    ids.sort((a, b) => (orderIndex.get(a) ?? 0) - (orderIndex.get(b) ?? 0));
  }

  const pos = new Map<string, { x: number; y: number }>();
  const depths = [...byDepth.keys()].sort((a, b) => a - b);
  for (const d of depths) {
    const row = byDepth.get(d) ?? [];
    row.forEach((id, i) => {
      pos.set(id, { x: i * COL_W, y: d * ROW_H });
    });
  }
  return pos;
}

export function maxLineageDepth(tasks: TaskNodeRecord[]): number {
  const m = taskDepthByUuid(tasks);
  let n = 0;
  for (const d of m.values()) if (d > n) n = d;
  return n;
}

/** Subtle fill behind nodes from sub-DAG depth (0 = none). */
export function depthBackground(depth: number): string | undefined {
  if (depth <= 0) return undefined;
  const t = Math.min(0.07 + depth * 0.035, 0.22);
  return `color-mix(in srgb, var(--color-primary) ${Math.round(t * 100)}%, transparent)`;
}

export function parentHint(tasks: TaskNodeRecord[], clientUuid: string): string | null {
  const by = taskMapByUuid(tasks);
  const t = by.get(clientUuid);
  const p = t?.parent_client_uuid?.trim();
  if (!p) return null;
  const pr = by.get(p)?.role?.trim();
  if (pr) return `↑ ${pr}`;
  return p.length > 12 ? `↑ ${p.slice(0, 8)}…` : `↑ ${p}`;
}
