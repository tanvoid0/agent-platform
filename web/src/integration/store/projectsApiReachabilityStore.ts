import { create } from 'zustand';

export type ProjectsApiReachability = 'checking' | 'online' | 'offline' | 'disabled';

interface State {
  status: ProjectsApiReachability;
  /** When the current in-flight check started (for "connecting… 3s"). */
  checkStartedAt: number | null;
  /** When the last check finished (success or failure). */
  lastCheckFinishedAt: number | null;
  lastOkAt: number | null;
  lastLatencyMs: number | null;
  lastError: string | null;
  beginCheck: () => void;
  recordResult: (result: { ok: boolean; latencyMs: number; error?: string }) => void;
  setDisabled: () => void;
}

export const useProjectsApiReachabilityStore = create<State>((set, get) => ({
  status: 'checking',
  checkStartedAt: null,
  lastCheckFinishedAt: null,
  lastOkAt: null,
  lastLatencyMs: null,
  lastError: null,
  beginCheck: () => {
    const prev = get().status;
    if (prev !== 'online') {
      set({ status: 'checking', checkStartedAt: Date.now() });
    } else {
      set({ checkStartedAt: Date.now() });
    }
  },
  recordResult: ({ ok, latencyMs, error }) => {
    const now = Date.now();
    if (ok) {
      set({
        status: 'online',
        checkStartedAt: null,
        lastCheckFinishedAt: now,
        lastOkAt: now,
        lastLatencyMs: latencyMs,
        lastError: null,
      });
    } else {
      set({
        status: 'offline',
        checkStartedAt: null,
        lastCheckFinishedAt: now,
        lastLatencyMs: latencyMs,
        lastError: error?.trim() || 'Request failed',
      });
    }
  },
  setDisabled: () =>
    set({
      status: 'disabled',
      checkStartedAt: null,
    }),
}));
