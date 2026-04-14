import { useEffect } from 'react';
import { hasRemoteProjectBackend, listRemoteProjects } from '@/integration/api/projectRemoteApi';
import { useProjectsApiReachabilityStore } from '@/integration/store/projectsApiReachabilityStore';

export function useProjectsReachabilityPolling(): void {
  useEffect(() => {
    const store = useProjectsApiReachabilityStore.getState();
    if (!hasRemoteProjectBackend()) {
      store.setDisabled();
      return;
    }
    let cancelled = false;
    const checkServer = async () => {
      if (cancelled) return;
      store.beginCheck();
      const t0 = performance.now();
      try {
        await listRemoteProjects(1, 0);
        if (cancelled) return;
        store.recordResult({ ok: true, latencyMs: Math.round(performance.now() - t0) });
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : String(e);
        store.recordResult({ ok: false, latencyMs: Math.round(performance.now() - t0), error: msg });
      }
    };
    void checkServer();
    const id = window.setInterval(() => void checkServer(), 15000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);
}
