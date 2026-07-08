export type ViewMode = "graph" | "board" | "timeline" | "events";

/** Default view segment for the process workspace (`/flow/graph`, …). */
export const DEFAULT_VIEW_MODE: ViewMode = "graph";

/** First path segment: `/flow/board`, `/flow/timeline/42`, etc. */
export const VIEW_MODE_PATH_SEGMENTS: ViewMode[] = ["graph", "board", "timeline", "events"];

/**
 * When pathname is `/board`, `/timeline/5`, etc. (relative to router basename), returns that view.
 * For `/teams` or unknown segments returns null.
 */
export function viewModeFromPathname(pathname: string): ViewMode | null {
  const first = pathname.split("/").filter(Boolean)[0];
  if (first && VIEW_MODE_PATH_SEGMENTS.includes(first as ViewMode)) {
    return first as ViewMode;
  }
  return null;
}

/** Canonical path: `/graph`, `/board/42`, … */
export function processWorkspacePath(view: ViewMode, processId: number | null): string {
  return processId != null ? `/${view}/${processId}` : `/${view}`;
}

export function isProcessWorkspacePath(pathname: string): boolean {
  return VIEW_MODE_PATH_SEGMENTS.some(
    (v) => pathname === `/${v}` || pathname.startsWith(`/${v}/`),
  );
}

/** Parse `processId` path segment; invalid or missing returns null. */
export function parseProcessIdParam(raw: string | undefined): number | null {
  if (raw == null || raw === "") return null;
  const n = parseInt(raw, 10);
  if (!Number.isFinite(n) || n <= 0) return null;
  return n;
}
