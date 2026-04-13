import type { SubagentNode, TaskNodeRecord } from "../api/types";
import type { BoardColumn } from "./dagTasks";
import { formatRelativeTimeFromMs } from "./formatRelativeTime";

export type BoardRow = {
  subagent: SubagentNode;
  task: TaskNodeRecord | null;
  column: BoardColumn;
};

export function matchesBoardSearch(row: BoardRow, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const { subagent } = row;
  const hay = [
    subagent.role,
    subagent.client_uuid,
    subagent.instructions ?? "",
    subagent.system_prompt ?? "",
  ]
    .join("\n")
    .toLowerCase();
  return hay.includes(q);
}

/** True when this row should appear in “needs attention” mode. */
export function rowMatchesNeedsAttention(row: BoardRow, processStatus: string | null): boolean {
  if (row.column === "awaiting_review") return true;
  if (row.column === "pending" && processStatus === "approval_required") return true;
  return false;
}

export function filterBoardRows(
  rows: BoardRow[],
  opts: {
    searchQuery: string;
    needsAttentionOnly: boolean;
    processStatus: string | null;
  },
): BoardRow[] {
  let out = rows;
  if (opts.searchQuery.trim()) {
    out = out.filter((r) => matchesBoardSearch(r, opts.searchQuery));
  }
  if (opts.needsAttentionOnly) {
    out = out.filter((r) => rowMatchesNeedsAttention(r, opts.processStatus));
  }
  return out;
}

/** Short label for status badge (matches board column names). */
export function boardStatusLabel(column: BoardColumn): string {
  const labels: Record<BoardColumn, string> = {
    pending: "Pending",
    running: "Running",
    awaiting_review: "Review",
    completed: "Done",
    failed: "Failed",
  };
  return labels[column];
}

/**
 * Relative activity from task timestamps: terminal uses completed_at (“Finished …”),
 * non-terminal uses started_at (“Started …”).
 */
export function relativeTaskActivity(row: BoardRow): string | null {
  const task = row.task;
  if (!task) return null;
  const col = row.column;
  const terminal = col === "completed" || col === "failed";
  const iso = terminal ? task.completed_at : task.started_at;
  if (!iso) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const rel = formatRelativeTimeFromMs(t);
  return terminal ? `Finished ${rel}` : `Started ${rel}`;
}
