import { useCallback, useMemo, useState } from "react";

import type { TaskNodeRecord } from "../api/types";
import { formatRelativeTimeFromIso } from "../lib/formatRelativeTime";
import { useProcessEventsQuery } from "../hooks/useProcessQueries";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const FILTERS = ["all", "trace", "status_change", "error"] as const;
export type EventLogFilter = (typeof FILTERS)[number];

function eventTypeVariant(
  t: string,
): "default" | "secondary" | "destructive" | "outline" {
  if (t === "error") return "destructive";
  if (t === "trace") return "secondary";
  if (t === "status_change") return "outline";
  return "secondary";
}

type Props = {
  processId: number | null;
  processStatus: string | undefined;
  tasks: TaskNodeRecord[];
  selectedUuid: string | null;
  onSelectUuid: (uuid: string | null) => void;
};

export function ProcessEventsView({
  processId,
  processStatus,
  tasks,
  selectedUuid,
  onSelectUuid,
}: Props) {
  const [filter, setFilter] = useState<EventLogFilter>("all");
  const [expanded, setExpanded] = useState<Record<number, boolean>>({});

  const eventsQuery = useProcessEventsQuery(processId, filter, processStatus);
  const events = eventsQuery.data?.events ?? [];

  const taskById = useMemo(() => {
    const m = new Map<number, TaskNodeRecord>();
    for (const t of tasks) m.set(t.id, t);
    return m;
  }, [tasks]);

  const toggleExpanded = useCallback((id: number) => {
    setExpanded((prev) => ({ ...prev, [id]: !prev[id] }));
  }, []);

  if (processId == null || processId <= 0) {
    return (
      <div className="text-muted-foreground flex min-h-[200px] flex-1 items-center justify-center px-4 text-center text-sm">
        Select or start a process to load its event log.
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-muted/30">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border/60 px-4 py-2">
        <span className="text-muted-foreground text-xs font-medium">Type</span>
        {FILTERS.map((f) => (
          <Button
            key={f}
            type="button"
            size="sm"
            variant={filter === f ? "default" : "outline"}
            className="h-8"
            onClick={() => setFilter(f)}
            aria-pressed={filter === f}
          >
            {f === "all" ? "All" : f.replace(/_/g, " ")}
          </Button>
        ))}
        {eventsQuery.isFetching && (
          <span className="text-muted-foreground text-xs">Updating…</span>
        )}
      </div>

      {eventsQuery.error && (
        <p className="text-destructive shrink-0 px-4 pt-2 text-sm">
          {eventsQuery.error instanceof Error ? eventsQuery.error.message : String(eventsQuery.error)}
        </p>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-2 pb-4">
        {eventsQuery.isLoading && (
          <p className="text-muted-foreground text-sm">Loading events…</p>
        )}
        {!eventsQuery.isLoading && events.length === 0 && (
          <div
            className="text-muted-foreground flex min-h-[160px] items-center justify-center rounded-lg border border-dashed border-border/80 px-4 text-center text-sm"
            role="status"
          >
            No events match this filter.
          </div>
        )}
        <ul className="space-y-2">
          {events.map((ev) => {
            const task = ev.task_id != null ? taskById.get(ev.task_id) : undefined;
            const rel = formatRelativeTimeFromIso(ev.created_at);
            const isOpen = !!expanded[ev.id];
            const long = ev.content.length > 280;
            const preview = long && !isOpen ? `${ev.content.slice(0, 280)}…` : ev.content;
            const rowSelected = task && selectedUuid === task.client_uuid;

            return (
              <li key={ev.id}>
                <div
                  role={task ? "button" : undefined}
                  tabIndex={task ? 0 : undefined}
                  className={cn(
                    "w-full rounded-lg border border-border/60 bg-background p-3 text-left text-sm shadow-sm transition-colors",
                    "hover:bg-muted/40",
                    rowSelected && "ring-2 ring-primary ring-offset-2 ring-offset-background",
                    task && "cursor-pointer",
                    !task && "cursor-default",
                  )}
                  onClick={() => {
                    if (task) onSelectUuid(task.client_uuid);
                  }}
                  onKeyDown={
                    task
                      ? (e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.preventDefault();
                            onSelectUuid(task.client_uuid);
                          }
                        }
                      : undefined
                  }
                >
                  <div className="flex flex-wrap items-center gap-2 gap-y-1">
                    <Badge variant={eventTypeVariant(ev.event_type)} className="font-mono text-[10px]">
                      {ev.event_type}
                    </Badge>
                    {rel && (
                      <span className="text-muted-foreground text-xs" title={ev.created_at}>
                        {rel}
                      </span>
                    )}
                    {task && (
                      <span className="text-muted-foreground text-xs">
                        · {task.role}{" "}
                        <span className="font-mono">({task.client_uuid.slice(0, 8)}…)</span>
                      </span>
                    )}
                    {ev.task_id != null && !task && (
                      <span className="text-muted-foreground font-mono text-xs">· task #{ev.task_id}</span>
                    )}
                  </div>
                  <pre
                    className={cn(
                      "mt-2 max-h-[min(40vh,24rem)] overflow-y-auto font-mono text-xs whitespace-pre-wrap break-words text-foreground",
                    )}
                  >
                    {preview}
                  </pre>
                  {long && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="mt-1 h-7 px-2 text-xs"
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleExpanded(ev.id);
                      }}
                    >
                      {isOpen ? "Show less" : "Show full"}
                    </Button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
