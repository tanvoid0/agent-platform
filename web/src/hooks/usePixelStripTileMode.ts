import { useCallback, useState } from "react";

const STORAGE_KEY = "agent-platform-pixel-strip-tiles";

export type PixelStripTileMode = "css" | "raster";

function readStored(): PixelStripTileMode {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "raster" || v === "css") return v;
  } catch {
    /* ignore */
  }
  return "css";
}

/** Persisted choice for task strip tiles: lightweight CSS chibi vs MIT pixel-agents rasters. */
export function usePixelStripTileMode(): [PixelStripTileMode, (m: PixelStripTileMode) => void] {
  const [mode, setModeState] = useState<PixelStripTileMode>(() =>
    typeof localStorage !== "undefined" ? readStored() : "css",
  );

  const setMode = useCallback((m: PixelStripTileMode) => {
    setModeState(m);
    try {
      localStorage.setItem(STORAGE_KEY, m);
    } catch {
      /* ignore */
    }
  }, []);

  return [mode, setMode];
}
