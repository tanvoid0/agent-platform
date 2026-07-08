import { useCallback, useEffect, useState } from "react";

const STORAGE_KEY = "agent-platform-default-project-id";
const CHANGE_EVENT = "agent-platform-default-project-id-change";

function readStored(): number | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v == null || v === "") return null;
    const n = parseInt(v, 10);
    return Number.isFinite(n) && n > 0 ? n : null;
  } catch {
    return null;
  }
}

/**
 * Persisted default project for “Start process” and Projects “Open” (localStorage).
 * Broadcasts on change so other hooks in the same tab stay in sync.
 */
export function useDefaultProjectId(): [number | null, (id: number | null) => void] {
  const [id, setIdState] = useState<number | null>(() =>
    typeof localStorage !== "undefined" ? readStored() : null,
  );

  useEffect(() => {
    function onStorage(e: StorageEvent) {
      if (e.key === STORAGE_KEY || e.key === null) setIdState(readStored());
    }
    function onCustom() {
      setIdState(readStored());
    }
    window.addEventListener("storage", onStorage);
    window.addEventListener(CHANGE_EVENT, onCustom);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener(CHANGE_EVENT, onCustom);
    };
  }, []);

  const setId = useCallback((next: number | null) => {
    setIdState(next);
    try {
      if (next == null) localStorage.removeItem(STORAGE_KEY);
      else localStorage.setItem(STORAGE_KEY, String(next));
    } catch {
      /* ignore */
    }
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }, []);

  return [id, setId];
}
