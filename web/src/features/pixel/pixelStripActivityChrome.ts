import type { PixelActivity } from "./pixelViewModel";
import { cn } from "@/lib/utils";

/** Strong strip-level cues so tiny sprites do not need to be “read” for motion. */
export function pixelStripActivityCellClass(activity: PixelActivity | undefined): string {
  switch (activity) {
    case "work":
      return cn(
        "border-2 border-blue-600/90 bg-blue-500/[0.14]",
        "shadow-[0_0_12px_-2px_rgba(37,99,235,0.65),inset_0_1px_0_0_rgba(255,255,255,0.12)]",
        "dark:border-blue-400 dark:bg-blue-500/25 dark:shadow-[0_0_14px_-2px_rgba(96,165,250,0.55)]",
      );
    case "walk":
      return cn(
        "border-2 border-amber-500/90 bg-amber-500/[0.14]",
        "shadow-[0_0_10px_-3px_rgba(245,158,11,0.5),inset_0_1px_0_0_rgba(255,255,255,0.08)]",
        "dark:border-amber-400 dark:bg-amber-500/20",
      );
    case "attention":
      return cn(
        "border-2 border-orange-500/95 bg-orange-500/[0.15]",
        "shadow-[0_0_12px_-2px_rgba(234,88,12,0.55),inset_0_1px_0_0_rgba(255,255,255,0.08)]",
        "dark:border-orange-400 dark:bg-orange-500/22",
      );
    case "idle":
    default:
      return cn(
        "border border-border/90 bg-muted/55",
        "shadow-[inset_0_0_0_1px_rgba(0,0,0,0.06)]",
        "dark:bg-muted/35 dark:shadow-[inset_0_0_0_1px_rgba(255,255,255,0.06)]",
      );
  }
}

export function pixelStripActivityShortLabel(activity: PixelActivity | undefined): string {
  switch (activity) {
    case "work":
      return "Working";
    case "walk":
      return "Ready";
    case "attention":
      return "Review";
    case "idle":
    default:
      return "Waiting";
  }
}

export function pixelStripActivityShortLabelClass(activity: PixelActivity | undefined): string {
  switch (activity) {
    case "work":
      return "text-blue-700 dark:text-blue-300";
    case "walk":
      return "text-amber-700 dark:text-amber-300";
    case "attention":
      return "text-orange-700 dark:text-orange-300";
    case "idle":
    default:
      return "text-muted-foreground";
  }
}
