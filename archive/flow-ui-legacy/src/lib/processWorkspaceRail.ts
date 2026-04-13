const INSPECTOR_WIDTH_STORAGE_KEY = "agent-platform:inspector-width-px";
export const DEFAULT_INSPECTOR_WIDTH_PX = 360;
export const INSPECTOR_WIDTH_MIN_PX = 260;
export const INSPECTOR_WIDTH_MAX_PX = 720;

export function readStoredInspectorWidth(): number {
  try {
    const raw = localStorage.getItem(INSPECTOR_WIDTH_STORAGE_KEY);
    const n = raw ? parseInt(raw, 10) : NaN;
    if (Number.isFinite(n) && n >= INSPECTOR_WIDTH_MIN_PX && n <= INSPECTOR_WIDTH_MAX_PX) {
      return n;
    }
  } catch {
    /* ignore */
  }
  return DEFAULT_INSPECTOR_WIDTH_PX;
}

export function persistInspectorWidthPx(width: number): void {
  try {
    localStorage.setItem(INSPECTOR_WIDTH_STORAGE_KEY, String(width));
  } catch {
    /* ignore */
  }
}

export function clampInspectorWidthPx(width: number, viewportWidth: number): number {
  const maxW = Math.min(INSPECTOR_WIDTH_MAX_PX, Math.floor(viewportWidth * 0.58));
  return Math.round(Math.min(maxW, Math.max(INSPECTOR_WIDTH_MIN_PX, width)));
}
