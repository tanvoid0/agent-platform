import type { RosterRole } from "../api/types";

/** Horizontal gap for Delegation-style wider roster cards */
const NODE_X = 300;
const NODE_Y = 112;

/**
 * Layer roles by parent edges (tree edges only). Roles with missing/invalid parents
 * are treated as roots. Unreachable nodes (e.g. cycles) are placed in a trailing row.
 */
export function rosterLineagePositions(roles: RosterRole[]): Map<string, { x: number; y: number }> {
  const byId = new Map(roles.map((r) => [r.id, r]));
  const children = new Map<string, string[]>();
  for (const r of roles) {
    const p = r.parent_id;
    if (p && byId.has(p)) {
      if (!children.has(p)) children.set(p, []);
      children.get(p)!.push(r.id);
    }
  }
  const roots = roles.filter((r) => !r.parent_id || !byId.has(r.parent_id));
  const pos = new Map<string, { x: number; y: number }>();
  let level = 0;
  let frontier = roots.map((r) => r.id);
  const assigned = new Set<string>();

  while (frontier.length) {
    const w = frontier.length;
    frontier.forEach((id, i) => {
      const x = (i - (w - 1) / 2) * NODE_X;
      const y = level * NODE_Y;
      pos.set(id, { x, y });
      assigned.add(id);
    });
    const next: string[] = [];
    for (const id of frontier) {
      for (const c of children.get(id) ?? []) {
        if (!assigned.has(c)) next.push(c);
      }
    }
    frontier = next;
    level++;
  }

  let orphanCol = 0;
  for (const r of roles) {
    if (!pos.has(r.id)) {
      pos.set(r.id, { x: orphanCol * NODE_X, y: level * NODE_Y });
      orphanCol++;
    }
  }
  return pos;
}
