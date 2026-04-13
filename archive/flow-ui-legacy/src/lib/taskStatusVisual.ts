import { normalizeTaskStatus, type BoardColumn } from "./dagTasks";

/** Hex accents for DAG edges, minimap, timeline/board cues — node borders use the same palette inline in `DagGraphView`. */
export const TASK_STATUS_COLORS: Record<BoardColumn, string> = {
  pending: "#94a3b8",
  running: "#2563eb",
  awaiting_review: "#d97706",
  completed: "#16a34a",
  failed: "#dc2626",
};

export function taskStatusColor(raw: string | undefined): string {
  return TASK_STATUS_COLORS[normalizeTaskStatus(raw ?? "pending")];
}
