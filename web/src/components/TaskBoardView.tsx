import { useCallback, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Copy, GitPullRequest } from "lucide-react";

import { boardRowsFromDag, taskDepthByUuid, type BoardColumn } from "../lib/dagTasks";
import type { PlannerDag, TaskNodeRecord } from "../api/types";
import {
  boardStatusLabel,
  filterBoardRows,
  relativeTaskActivity,
  type BoardRow,
} from "../lib/taskBoardFilter";
import { taskStatusColor } from "../lib/taskStatusVisual";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

const COLS: {
  key: BoardColumn;
  label: string;
  hint: string;
}[] = [
  { key: "pending", label: "Pending", hint: "Not started" },
  { key: "running", label: "Running", hint: "In progress" },
  { key: "awaiting_review", label: "Review", hint: "Needs your review" },
  { key: "completed", label: "Completed", hint: "Done" },
  { key: "failed", label: "Failed", hint: "Errors" },
];

function shortUuid(uuid: string): string {
  const t = uuid.replace(/-/g, "");
  return t.length >= 8 ? t.slice(0, 8) : uuid.slice(0, 8);
}

/** Aligns with Kanban “open audit” affordances: review queue, plan approval, or finished work. */
function rowCanOpenReview(row: BoardRow, processStatus: string | null): boolean {
  if (row.column === "awaiting_review") return true;
  if (row.column === "pending" && processStatus === "approval_required") return true;
  if (row.column === "completed") return true;
  return false;
}

function reviewActionTitle(row: BoardRow, processStatus: string | null): string {
  if (row.column === "pending" && processStatus === "approval_required") {
    return "Approve or review plan (sidebar)";
  }
  if (row.column === "awaiting_review") return "Open review modal";
  if (row.column === "completed") return "View task details in sidebar";
  return "Open in sidebar";
}

type Props = {
  dag: PlannerDag | null;
  tasks: TaskNodeRecord[];
  selectedUuid: string | null;
  onSelectUuid: (uuid: string | null) => void;
  processStatus: string | null;
  onRetryFailedTask?: (taskId: number) => void;
  retryTaskPending?: boolean;
  retryProcessPending?: boolean;
  /** Select task and open the review modal (board: Review column). */
  onOpenTaskReview?: (uuid: string) => void;
};

export function TaskBoardView({
  dag,
  tasks,
  selectedUuid,
  onSelectUuid,
  processStatus,
  onRetryFailedTask,
  retryTaskPending,
  retryProcessPending,
  onOpenTaskReview,
}: Props) {
  const rows = boardRowsFromDag(dag, tasks) as BoardRow[];
  const depthByUuid = taskDepthByUuid(tasks);
  const [search, setSearch] = useState("");
  const [needsAttention, setNeedsAttention] = useState(false);
  const [expandedUuid, setExpandedUuid] = useState<Record<string, boolean>>({});

  const filteredRows = useMemo(
    () =>
      filterBoardRows(rows, {
        searchQuery: search,
        needsAttentionOnly: needsAttention,
        processStatus,
      }),
    [rows, search, needsAttention, processStatus],
  );

  const filterActive = search.trim().length > 0 || needsAttention;
  const clearFilters = useCallback(() => {
    setSearch("");
    setNeedsAttention(false);
  }, []);

  const toggleExpanded = useCallback((id: string) => {
    setExpandedUuid((prev) => ({ ...prev, [id]: !prev[id] }));
  }, []);

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-muted/30">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border/60 px-4 py-2">
        <Input
          type="search"
          placeholder="Search role, UUID, instructions…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="max-w-xs min-w-[12rem]"
          aria-label="Search tasks"
        />
        <Button
          type="button"
          variant={needsAttention ? "default" : "outline"}
          size="sm"
          className="h-8"
          onClick={() => setNeedsAttention((v) => !v)}
          aria-pressed={needsAttention}
        >
          Needs attention
        </Button>
        {filterActive && (
          <>
            <span className="text-muted-foreground text-xs">
              Showing {filteredRows.length} of {rows.length}
            </span>
            <Button type="button" variant="ghost" size="sm" className="h-8" onClick={clearFilters}>
              Clear
            </Button>
          </>
        )}
      </div>

      <div className="task-board-scroll min-h-0 flex-1 overflow-x-auto overflow-y-auto px-4 py-2 pb-4">
        <div className="flex min-h-[280px] items-start gap-3">
          {COLS.map((col) => {
            const colRows = filteredRows.filter((r) => r.column === col.key);
            const tint = taskStatusColor(col.key);
            return (
              <div
                key={col.key}
                className="board-column w-[min(100%,var(--board-col-w))] shrink-0 rounded-lg border border-border/60 p-2 shadow-sm transition-shadow [--board-col-w:11.5rem] sm:[--board-col-w:12.5rem]"
                style={{
                  backgroundColor: `color-mix(in srgb, ${tint} 7%, var(--color-muted))`,
                }}
              >
                <div className="mb-2">
                  <div className="flex items-center gap-2 text-xs font-semibold text-muted-foreground">
                    <span
                      className={cn(
                        "size-2 shrink-0 rounded-full",
                        col.key === "running" && "animate-pulse",
                      )}
                      style={{ backgroundColor: taskStatusColor(col.key) }}
                      aria-hidden
                    />
                    <span className="min-w-0 truncate">{col.label}</span>
                    <Badge variant="secondary" className="ml-auto h-5 min-w-5 px-1.5 font-mono text-[10px]">
                      {colRows.length}
                    </Badge>
                  </div>
                  <p className="text-muted-foreground mt-0.5 pl-4 text-[10px] leading-tight">{col.hint}</p>
                </div>

                {colRows.length === 0 ? (
                  <div
                    className="text-muted-foreground flex min-h-[120px] items-center justify-center rounded-md border border-dashed border-border/80 px-2 py-6 text-center text-xs"
                    role="status"
                  >
                    No tasks in this state
                  </div>
                ) : (
                  <div className="flex flex-col gap-1.5">
                    {colRows.map((r) => {
                      const id = r.subagent.client_uuid;
                      const selected = selectedUuid === id;
                      const tokens = r.task?.tokens_used ?? 0;
                      const depth = depthByUuid.get(id) ?? 0;
                      const statusColor = taskStatusColor(r.task?.status);
                      const instructions = r.subagent.instructions?.trim() ?? "";
                      const expanded = !!expandedUuid[id];
                      const activity = relativeTaskActivity(r);
                      const running = r.column === "running";

                      return (
                        <div key={id} className="group relative">
                          <div
                            role="button"
                            tabIndex={0}
                            data-selected={selected ? "" : undefined}
                            title={
                              instructions.length > 0
                                ? `${r.subagent.role} — ${instructions.slice(0, 200)}${instructions.length > 200 ? "…" : ""}`
                                : `${r.subagent.role} — ${id}`
                            }
                            className={cn(
                              buttonVariants({ variant: "outline", size: "default" }),
                              "h-auto min-h-0 w-full flex-col items-stretch gap-1 border-l-[3px] py-2 pr-8 text-left text-sm font-normal whitespace-normal transition-[box-shadow,transform,border-color] duration-150",
                              "hover:-translate-y-px hover:shadow-md hover:border-border/90",
                              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
                              selected &&
                                "border-primary ring-2 ring-primary ring-offset-2 ring-offset-background",
                            )}
                            style={{
                              borderLeftColor: statusColor,
                              marginLeft: depth * 12,
                              paddingLeft: 10,
                              maxWidth: "100%",
                            }}
                            onClick={() => onSelectUuid(id)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onSelectUuid(id);
                              }
                            }}
                            aria-label={`${r.subagent.role}, ${boardStatusLabel(r.column)}`}
                          >
                            <div className="flex flex-wrap items-center gap-1.5">
                              {running && (
                                <span
                                  className="size-2 shrink-0 animate-pulse rounded-full"
                                  style={{ backgroundColor: statusColor }}
                                  aria-hidden
                                />
                              )}
                              <span className="font-semibold text-foreground">{r.subagent.role}</span>
                              {depth > 0 && (
                                <span
                                  className="text-muted-foreground shrink-0 rounded border border-border/80 bg-muted/50 px-1 font-mono text-[9px] leading-none"
                                  title={`Sub-task depth ${depth}`}
                                >
                                  d{depth}
                                </span>
                              )}
                              <Badge
                                variant="outline"
                                className="h-5 px-1.5 text-[10px] font-medium uppercase tracking-wide"
                                style={{
                                  borderColor: statusColor,
                                  color: "var(--color-foreground)",
                                }}
                              >
                                {boardStatusLabel(r.column)}
                              </Badge>
                            </div>

                            <div className="text-muted-foreground flex items-center gap-1 font-mono text-xs">
                              <span className="min-w-0 truncate" title={id}>
                                {shortUuid(id)}
                              </span>
                              {activity && (
                                <span className="text-muted-foreground shrink-0 text-[10px] not-italic">
                                  · {activity}
                                </span>
                              )}
                            </div>

                            {instructions.length > 0 && (
                              <div className="border-border/60 mt-0.5 border-t pt-1">
                                <button
                                  type="button"
                                  className="text-muted-foreground hover:text-foreground flex w-full items-center gap-0.5 text-left text-[10px] font-medium"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    toggleExpanded(id);
                                  }}
                                  aria-expanded={expanded}
                                >
                                  {expanded ? (
                                    <ChevronDown className="size-3 shrink-0" aria-hidden />
                                  ) : (
                                    <ChevronRight className="size-3 shrink-0" aria-hidden />
                                  )}
                                  Instructions
                                </button>
                                <p
                                  className={cn(
                                    "text-muted-foreground mt-0.5 text-xs leading-snug",
                                    !expanded && "line-clamp-2",
                                  )}
                                >
                                  {instructions}
                                </p>
                              </div>
                            )}

                            {tokens > 0 && (
                              <div className="text-muted-foreground text-xs">{tokens} tok</div>
                            )}

                            {rowCanOpenReview(r, processStatus) && (
                              <div className="border-border/60 mt-1 flex items-center justify-end border-t pt-1.5">
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="icon-xs"
                                  className="group/audit text-muted-foreground hover:bg-emerald-50 hover:text-emerald-600 dark:hover:bg-emerald-950/40 dark:hover:text-emerald-400"
                                  title={reviewActionTitle(r, processStatus)}
                                  aria-label={`${reviewActionTitle(r, processStatus)}: ${r.subagent.role}`}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    if (r.column === "awaiting_review" && onOpenTaskReview) {
                                      onOpenTaskReview(id);
                                    } else {
                                      onSelectUuid(id);
                                    }
                                  }}
                                >
                                  <GitPullRequest className="size-3.5" strokeWidth={2.5} aria-hidden />
                                </Button>
                              </div>
                            )}

                            {col.key === "failed" &&
                              processStatus === "failed" &&
                              onRetryFailedTask &&
                              r.task && (
                                <Button
                                  type="button"
                                  variant="secondary"
                                  size="sm"
                                  className="mt-1 h-7 w-full text-xs"
                                  disabled={retryTaskPending || retryProcessPending}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    onRetryFailedTask(r.task!.id);
                                  }}
                                >
                                  Retry task
                                </Button>
                              )}
                          </div>

                          <button
                            type="button"
                            className="text-muted-foreground hover:text-foreground absolute top-2 right-2 rounded p-0.5 hover:bg-muted"
                            aria-label={`Copy UUID ${id}`}
                            onClick={(e) => {
                              e.stopPropagation();
                              void navigator.clipboard?.writeText(id);
                            }}
                          >
                            <Copy className="size-3.5" aria-hidden />
                          </button>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
