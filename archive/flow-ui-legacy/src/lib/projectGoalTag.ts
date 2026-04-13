import type { ProcessRecord } from "../api/types";

/** Leading `[tag] ` in goals — used for grouping and new-process prefix. */
export function projectTagFromGoal(goal: string): string | null {
  const m = /^\[([^\]]+)\]\s/.exec(goal.trim());
  return m ? m[1] : null;
}

export function uniqueSortedProjectTags(processes: ProcessRecord[]): string[] {
  const s = new Set<string>();
  for (const r of processes) {
    const t = projectTagFromGoal(r.goal);
    if (t) s.add(t);
  }
  return Array.from(s).sort((a, b) => a.localeCompare(b));
}
