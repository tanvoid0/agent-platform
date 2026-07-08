import { buildTimelineRows } from "../lib/dagTasks";
import type { PlannerDag, TaskNodeRecord } from "../api/types";
import { taskStatusColor } from "../lib/taskStatusVisual";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

type Props = {
  dag: PlannerDag | null;
  tasks: TaskNodeRecord[];
  selectedUuid: string | null;
  onSelectUuid: (uuid: string | null) => void;
};

export function TaskTimelineView({ dag, tasks, selectedUuid, onSelectUuid }: Props) {
  const rows = buildTimelineRows(dag, tasks);

  if (rows.length === 0) {
    return <p className="text-muted-foreground mx-4 my-4 text-sm">No tasks yet.</p>;
  }

  const byWave = new Map<number, typeof rows>();
  for (const r of rows) {
    const list = byWave.get(r.waveIndex) ?? [];
    list.push(r);
    byWave.set(r.waveIndex, list);
  }
  const waves = [...byWave.keys()].sort((a, b) => a - b);

  return (
    <div className="flex-1 overflow-auto bg-muted/30 px-4 py-2 pb-4">
      {waves.map((w) => (
        <div
          key={w}
          className="timeline-wave mb-4 rounded-lg border border-border/80 bg-card/40 p-2 shadow-sm"
        >
          <div className="mb-2 border-b border-border/60 pb-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            Wave {w}
          </div>
          <ul className="list-none space-y-0.5 pl-0">
            {(byWave.get(w) ?? []).map((r) => {
              const selected = selectedUuid === r.client_uuid;
              const dotColor = taskStatusColor(r.taskStatus);
              const pulse = r.taskStatus.toLowerCase() === "running";
              return (
                <li key={r.client_uuid}>
                  <Button
                    type="button"
                    variant="ghost"
                    className={cn(
                      "h-auto min-h-0 w-full max-w-full justify-start gap-2 rounded-md px-2 py-1.5 text-sm font-normal transition-colors",
                      selected && "bg-accent text-accent-foreground",
                    )}
                    onClick={() => onSelectUuid(r.client_uuid)}
                  >
                    <span
                      className={cn(
                        "size-2 shrink-0 rounded-full",
                        pulse && "animate-pulse",
                      )}
                      style={{ backgroundColor: dotColor }}
                      aria-hidden
                    />
                    <span className="min-w-0 flex-1 truncate text-left">
                      <span className="font-medium">{r.role}</span>
                      <span className="text-muted-foreground"> · </span>
                      <span>{r.taskStatus}</span>
                      <span className="text-muted-foreground text-xs"> · {r.client_uuid}</span>
                    </span>
                  </Button>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </div>
  );
}
