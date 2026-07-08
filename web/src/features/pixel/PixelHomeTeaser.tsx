import type { BoardColumn } from "../../lib/dagTasks";
import { TASK_STATUS_COLORS } from "../../lib/taskStatusVisual";
import { useReducedMotion } from "./useReducedMotion";

const DEMO_TILES: BoardColumn[] = [
  "pending",
  "pending",
  "running",
  "running",
  "awaiting_review",
  "completed",
  "pending",
  "running",
  "completed",
  "completed",
  "failed",
  "pending",
  "running",
  "awaiting_review",
  "completed",
  "running",
  "completed",
  "pending",
  "pending",
  "running",
  "completed",
  "awaiting_review",
  "completed",
  "completed",
];

/**
 * Static pixel-style strip on the home route when no process is loaded.
 * Mirrors {@link PixelProcessStrip} chrome and task-status palette (read-only demo).
 */
export default function PixelHomeTeaser() {
  const reducedMotion = useReducedMotion();
  return (
    <div className="pixel-home-teaser border-border/60 bg-muted/20 mb-2 flex flex-col gap-1.5 rounded-md border px-2 py-1.5 sm:flex-row sm:flex-wrap sm:items-center sm:gap-2">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <div className="text-muted-foreground shrink-0 font-mono text-[10px] uppercase tracking-wide">
          Process
        </div>
        <div
          className="border-border shrink-0 rounded border bg-card px-1.5 py-0.5 font-mono text-[10px] leading-none text-muted-foreground"
          style={{ boxShadow: "inset 0 0 0 1px color-mix(in srgb, currentColor 12%, transparent)" }}
        >
          preview
        </div>
        <div className="bg-border hidden h-4 w-px sm:block" aria-hidden />
        <div className="flex min-w-0 flex-1 flex-wrap gap-px opacity-95" aria-hidden>
          {DEMO_TILES.map((col, i) => {
            const color = TASK_STATUS_COLORS[col];
            const pulse = !reducedMotion && col === "running" && i % 3 === 1;
            return (
              <span
                key={i}
                className={`inline-block size-[10px] rounded-[1px] border border-black/20 dark:border-white/15 ${pulse ? "animate-pulse" : ""}`}
                style={{ backgroundColor: color }}
                title={col}
              />
            );
          })}
        </div>
      </div>
      <p className="text-muted-foreground text-[10px] leading-snug sm:ml-auto sm:max-w-[60%] sm:text-right">
        Load or start a process — live task tiles appear above the summary in the{" "}
        <a
          href="#process-workspace"
          className="text-primary font-medium underline underline-offset-2 hover:no-underline"
        >
          process workspace
        </a>
        .{" "}
        <a
          href="#recent-processes"
          className="text-primary font-medium underline underline-offset-2 hover:no-underline"
        >
          Jump to recent processes
        </a>
      </p>
    </div>
  );
}
