import { useMemo } from "react";

import type { ProcessRecord } from "../api/types";
import { formatRelativeTimeFromIso } from "../lib/formatRelativeTime";
import { projectTagFromGoal } from "../lib/projectGoalTag";
import { processStatusBadgeVariant } from "../lib/processStatusBadge";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

type Props = {
  onPickProcess: (id: number) => void;
  selectedProcessId?: number | null;
  className?: string;
  processes: ProcessRecord[];
  listLoading: boolean;
  listError: Error | null;
  /** Empty: show all. Otherwise only goals with this leading `[tag]`. */
  projectTagFilter?: string;
};

const GOAL_PREVIEW = 72;

export function RecentProcessesList({
  onPickProcess,
  selectedProcessId,
  className,
  processes,
  listLoading,
  listError,
  projectTagFilter = "",
}: Props) {
  const listSnippet = useMemo(() => {
    const tag = projectTagFilter.trim();
    const rows = tag
      ? processes.filter((r) => projectTagFromGoal(r.goal) === tag)
      : processes;
    return rows.slice(0, 15);
  }, [processes, projectTagFilter]);

  return (
    <div className={cn("space-y-2", className)}>
      <div className="text-sm font-semibold">Recent processes</div>
      {listLoading && (
        <span className="text-sm text-muted-foreground">Loading list…</span>
      )}
      {listError && <p className="text-destructive">{listError.message}</p>}
      <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
        {listSnippet.map((r) => {
          const rel = formatRelativeTimeFromIso(r.created_at);
          const selected = selectedProcessId != null && selectedProcessId === r.id;
          const goal =
            r.goal.length > GOAL_PREVIEW ? `${r.goal.slice(0, GOAL_PREVIEW)}…` : r.goal;

          return (
            <li key={r.id}>
              <button
                type="button"
                onClick={() => onPickProcess(r.id)}
                className={cn(
                  "flex w-full flex-col gap-1.5 rounded-lg border border-border/80 bg-card p-3 text-left text-sm shadow-sm transition-colors",
                  "hover:border-border hover:bg-muted/30",
                  selected && "ring-2 ring-primary ring-offset-2 ring-offset-background",
                )}
              >
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-mono text-xs font-semibold tabular-nums text-foreground">
                    #{r.id}
                  </span>
                  <Badge variant={processStatusBadgeVariant(r.status)} className="font-mono text-[10px]">
                    {r.status}
                  </Badge>
                  {rel && (
                    <span className="text-muted-foreground ml-auto text-xs" title={r.created_at}>
                      {rel}
                    </span>
                  )}
                </div>
                <p className="text-muted-foreground line-clamp-2 text-xs leading-snug" title={r.goal}>
                  {goal}
                </p>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
