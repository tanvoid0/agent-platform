import { GitBranch, ListOrdered } from "lucide-react";
import { useMemo, useState } from "react";

import { plannerTopologicalUuids, shortUuid } from "../api/dag";
import type { PlannerDag } from "../api/types";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { cn } from "@/lib/utils";

function truncate(s: string, max: number): string {
  const t = s.trim();
  if (t.length <= max) return t;
  return `${t.slice(0, max).trim()}…`;
}

type Props = {
  dag: PlannerDag;
  className?: string;
};

export function PlannerDagPreview({ dag, className }: Props) {
  const [goalOpen, setGoalOpen] = useState(false);
  const order = useMemo(() => plannerTopologicalUuids(dag), [dag]);
  const roleByUuid = useMemo(() => {
    const m = new Map<string, string>();
    for (const s of dag.subagents) m.set(s.client_uuid, s.role);
    return m;
  }, [dag]);

  const byUuid = useMemo(() => {
    const m = new Map(dag.subagents.map((s) => [s.client_uuid, s]));
    return m;
  }, [dag]);

  const goalPreview = truncate(dag.goal_restatement, goalOpen ? 10_000 : 220);

  return (
    <div className={cn("space-y-3", className)}>
      <Card size="sm" className="ring-sky-500/20">
        <CardHeader className="pb-0">
          <div className="flex flex-wrap items-start justify-between gap-2">
            <div className="min-w-0 space-y-1">
              <CardTitle className="text-sm">{dag.team_name}</CardTitle>
              <CardDescription className="text-xs">Planner goal (restatement)</CardDescription>
            </div>
            <Badge variant="secondary" className="shrink-0 font-mono text-[10px]">
              {dag.subagents.length} subagent{dag.subagents.length === 1 ? "" : "s"}
            </Badge>
          </div>
        </CardHeader>
        <CardContent className="space-y-2 pt-2">
          <p className="text-foreground text-sm leading-relaxed whitespace-pre-wrap">
            {goalPreview}
          </p>
          {dag.goal_restatement.trim().length > 220 && (
            <button
              type="button"
              className="text-primary text-xs font-medium underline-offset-4 hover:underline"
              onClick={() => setGoalOpen((o) => !o)}
            >
              {goalOpen ? "Show less" : "Show full goal"}
            </button>
          )}
        </CardContent>
      </Card>

      <div className="flex items-center gap-2 text-muted-foreground text-xs">
        <ListOrdered className="size-3.5 shrink-0" aria-hidden />
        <span>
          Suggested execution order (dependencies respected). Click nodes in the Graph tab for execution
          status once tasks exist.
        </span>
      </div>

      <ol className="list-none space-y-2 p-0">
        {order.map((uuid, idx) => {
          const s = byUuid.get(uuid);
          if (!s) return null;
          const deps = s.dependencies ?? [];
          return (
            <li key={uuid}>
              <Card
                size="sm"
                className="border-border/80 bg-muted/20 py-3 ring-foreground/8"
              >
                <CardContent className="space-y-2 px-3 py-0">
                  <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                    <span className="text-muted-foreground font-mono text-[10px] tabular-nums">
                      {idx + 1}.
                    </span>
                    <span className="text-foreground font-medium leading-snug">{s.role}</span>
                    <span className="text-muted-foreground font-mono text-[10px]">
                      {shortUuid(s.client_uuid)}
                    </span>
                  </div>
                  {deps.length > 0 && (
                    <div className="flex flex-wrap items-center gap-1.5">
                      <GitBranch className="text-muted-foreground size-3 shrink-0" aria-hidden />
                      <span className="text-muted-foreground text-[10px]">After</span>
                      {deps.map((d) => (
                        <Badge key={d} variant="outline" className="font-normal text-[10px]">
                          {roleByUuid.get(d) ?? shortUuid(d)}
                        </Badge>
                      ))}
                    </div>
                  )}
                  {deps.length === 0 && (
                    <p className="text-muted-foreground text-[10px]">No dependencies (root task)</p>
                  )}
                  <p className="text-foreground/90 border-border/60 border-l-2 pl-2 text-xs leading-relaxed">
                    {truncate(s.instructions, 360)}
                  </p>
                  <div className="flex flex-wrap gap-1.5">
                    {s.model ? (
                      <Badge variant="secondary" className="font-mono text-[10px]">
                        {s.model}
                      </Badge>
                    ) : null}
                    {s.subdecompose ? (
                      <Badge className="text-[10px]">sub-decompose</Badge>
                    ) : null}
                    {s.requires_review ? (
                      <Badge variant="outline" className="border-amber-500/40 text-[10px] text-amber-900 dark:text-amber-100">
                        review gate
                      </Badge>
                    ) : null}
                  </div>
                </CardContent>
              </Card>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
