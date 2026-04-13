import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

import type { PlannerDag, ProcessStatus, TaskNodeRecord } from "../../api/types";
import { boardRowsForPixelStrip } from "../../lib/dagTasks";
import { taskStatusColor } from "../../lib/taskStatusVisual";
import PixelOfficeCanvas from "./PixelOfficeCanvas";
import PixelOfficeFullCanvas from "./PixelOfficeFullCanvas";
import { PixelErrorBoundary } from "./PixelErrorBoundary";
import { PixelChibiTile } from "./PixelChibiTile";
import { PixelRasterChibiTile } from "./PixelRasterChibiTile";
import { PixelStripRafProvider } from "./PixelStripRafContext";
import {
  pixelStripActivityCellClass,
  pixelStripActivityShortLabel,
  pixelStripActivityShortLabelClass,
} from "./pixelStripActivityChrome";
import { buildPixelViewModel, type PixelEventHints } from "./pixelViewModel";
import { useReducedMotion } from "./useReducedMotion";
import { cn } from "@/lib/utils";
import type { PixelStripTileMode } from "../../hooks/usePixelStripTileMode";
import { Button } from "@/components/ui/button";

type Props = {
  processStatus: ProcessStatus;
  dag: PlannerDag | null;
  tasks: TaskNodeRecord[];
  /** Optional bounded hints from the parent (e.g. last event types); reserved for finer animation. */
  eventHints?: PixelEventHints;
  /** When set, matches roster `accent_color` to subagent `role` for per-agent torso tint. */
  teamSnapshotJson?: string | null;
  /** When true, pixel office canvas stays off (e.g. 3D boundary spike is active in Process controls). */
  pixelOfficeLocked?: boolean;
  /** Task row tiles: CSS chibi (default) or MIT sprite sheets (falls back to CSS if assets missing). */
  stripTileMode?: PixelStripTileMode;
};

/**
 * Server-driven process preview: status tiles + optional Canvas activity strip (read-only).
 * See `README.md` in this folder and ADR 0003.
 */
function PixelProcessStripInner({
  processStatus,
  dag,
  tasks,
  eventHints,
  teamSnapshotJson,
  pixelOfficeLocked = false,
  stripTileMode = "css",
}: Props) {
  const rows = useMemo(
    () => boardRowsForPixelStrip(dag, tasks, teamSnapshotJson),
    [dag, tasks, teamSnapshotJson],
  );
  const scene = useMemo(
    () => buildPixelViewModel(processStatus, dag, tasks, eventHints, teamSnapshotJson),
    [processStatus, dag, tasks, eventHints, teamSnapshotJson],
  );
  const reducedMotion = useReducedMotion();
  const [canvasOpen, setCanvasOpen] = useState(false);
  /** `full` = pixel-agents `default-layout-1.json` (multi-room office). `mini` = tiny 4-desk placeholder only. */
  const [officeMode, setOfficeMode] = useState<"mini" | "full">("full");

  useEffect(() => {
    if (pixelOfficeLocked) setCanvasOpen(false);
  }, [pixelOfficeLocked]);

  const taskTiles = (
    <>
      {rows.map((r, i) => {
        const id = r.subagent.client_uuid;
        const color = taskStatusColor(r.task?.status ?? r.column);
        const agentVm = scene.agents[i];
        const statusLabel =
          agentVm?.activity === "work"
            ? "working"
            : agentVm?.activity === "walk"
              ? "ready / queued"
              : agentVm?.activity === "attention"
                ? "needs attention"
                : "waiting";
        const label = `${r.subagent.role} · ${r.column} · ${statusLabel}`;
        const pulse = !reducedMotion && agentVm?.activity === "work";
        const short = pixelStripActivityShortLabel(agentVm?.activity);
        return (
          <span
            key={id}
            role="listitem"
            aria-label={`${r.subagent.role}, ${short}`}
            className={cn(
              "inline-flex flex-col items-center justify-center gap-0.5 rounded-md px-0.5 pt-0.5 pb-0.5 transition-[box-shadow,background-color,border-color] duration-200",
              pixelStripActivityCellClass(agentVm?.activity),
              !reducedMotion && agentVm?.activity === "work" && "motion-safe:animate-pulse",
            )}
          >
            {stripTileMode === "raster" && agentVm ? (
              <PixelRasterChibiTile agent={agentVm} statusColor={color} title={label} pulse={pulse} />
            ) : (
              <PixelChibiTile color={color} title={label} pulse={pulse} />
            )}
            <span className="text-muted-foreground max-w-[4.5rem] truncate text-center font-mono text-[8px] leading-none">
              {r.subagent.role}
            </span>
            <span
              className={cn(
                "max-w-[4.5rem] truncate text-center font-mono text-[7px] font-semibold uppercase leading-none tracking-tight",
                pixelStripActivityShortLabelClass(agentVm?.activity),
              )}
            >
              {short}
            </span>
          </span>
        );
      })}
      {rows.length === 0 && (
        <span className="text-muted-foreground text-xs italic">No tasks yet</span>
      )}
    </>
  );

  return (
    <div
      className="pixel-process-strip border-border/60 bg-muted/20 mb-2 flex flex-col gap-1.5 rounded-md border px-2 py-1.5"
      title="One tile per agent: sprite + shirt color are stable per agent id; animation reflects working vs waiting. Ready tasks start in server FIFO order when AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS is set."
    >
      <div className="flex flex-wrap items-center gap-2">
        <div className="text-muted-foreground shrink-0 font-mono text-[10px] uppercase tracking-wide">
          Process
        </div>
        <div
          className="pixel-process-strip__status border-border shrink-0 rounded border bg-card px-1.5 py-0.5 font-mono text-[10px] leading-none"
          style={{ boxShadow: "inset 0 0 0 1px color-mix(in srgb, currentColor 12%, transparent)" }}
        >
          {processStatus}
        </div>
        <div className="bg-border mx-0.5 hidden h-4 w-px sm:block" aria-hidden />
        {stripTileMode === "raster" ? (
          <PixelStripRafProvider>
            <div
              className="grid min-w-0 flex-1 grid-cols-[repeat(auto-fill,minmax(2.75rem,1fr))] gap-x-1 gap-y-1.5 sm:grid-cols-6"
              role="list"
              aria-label="Team agents (row by row grid)"
            >
              {taskTiles}
            </div>
          </PixelStripRafProvider>
        ) : (
          <div
            className="grid min-w-0 flex-1 grid-cols-[repeat(auto-fill,minmax(2.75rem,1fr))] gap-x-1 gap-y-1.5 sm:grid-cols-6"
            role="list"
            aria-label="Team agents (row by row grid)"
          >
            {taskTiles}
          </div>
        )}
        {!reducedMotion && (
          <>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="text-muted-foreground h-7 shrink-0 gap-0.5 px-1.5 text-[10px]"
              disabled={pixelOfficeLocked}
              title={
                pixelOfficeLocked
                  ? "Turn off the 3D boundary visualization in Process controls to open the pixel office."
                  : undefined
              }
              onClick={() => setCanvasOpen((o) => !o)}
              aria-expanded={canvasOpen}
              aria-controls="pixel-office-canvas-wrap"
            >
              {canvasOpen ? <ChevronDown className="size-3" aria-hidden /> : <ChevronRight className="size-3" aria-hidden />}
              Pixel office
            </Button>
            {canvasOpen && !pixelOfficeLocked && (
              <div
                className="border-border/60 flex shrink-0 items-center gap-0.5 rounded border bg-muted/30 p-0.5"
                role="group"
                aria-label="Pixel office layout"
              >
                <Button
                  type="button"
                  variant={officeMode === "full" ? "secondary" : "ghost"}
                  size="sm"
                  className="h-6 px-1.5 text-[10px]"
                  onClick={() => setOfficeMode("full")}
                  title="Full office from pixel-agents layout JSON (furniture + zones)"
                >
                  Full office
                </Button>
                <Button
                  type="button"
                  variant={officeMode === "mini" ? "secondary" : "ghost"}
                  size="sm"
                  className="h-6 px-1.5 text-[10px]"
                  onClick={() => setOfficeMode("mini")}
                  title="Compact 4-desk checkerboard (legacy preview, not the extension layout)"
                >
                  Compact
                </Button>
              </div>
            )}
          </>
        )}
      </div>
      {!reducedMotion && canvasOpen && !pixelOfficeLocked && (
        <div id="pixel-office-canvas-wrap" className="border-border/50 min-w-0 overflow-hidden rounded border bg-black/5 dark:bg-white/5">
          {officeMode === "full" ? (
            <PixelOfficeFullCanvas scene={scene} className="w-full" />
          ) : (
            <PixelOfficeCanvas scene={scene} className="w-full" />
          )}
        </div>
      )}
    </div>
  );
}

export default function PixelProcessStrip(props: Props) {
  return (
    <PixelErrorBoundary>
      <PixelProcessStripInner {...props} />
    </PixelErrorBoundary>
  );
}
