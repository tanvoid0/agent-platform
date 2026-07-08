import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { apiUrl } from "../api/client";
import { queryKeys } from "../query/keys";

/**
 * Subscribes to SSE log tail; invalidates process detail (and list) so UI matches GET /processes/:id.
 */
export function useProcessEventStream(processId: number | null, enabled: boolean): void {
  const queryClient = useQueryClient();
  const attemptRef = useRef(0);

  useEffect(() => {
    if (processId == null || processId <= 0 || !enabled) return;

    const url = apiUrl(`/processes/${encodeURIComponent(String(processId))}/stream`);
    let cancelled = false;
    let es: EventSource | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;

    const invalidate = () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.processes.detail(processId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.processes.all });
    };

    const open = () => {
      if (cancelled) return;
      es = new EventSource(url);

      es.onopen = () => {
        attemptRef.current = 0;
      };

      es.onmessage = () => {
        attemptRef.current = 0;
        invalidate();
      };

      es.onerror = () => {
        es?.close();
        es = null;
        if (cancelled) return;
        attemptRef.current += 1;
        const delay = Math.min(
          30_000,
          500 * 2 ** Math.min(attemptRef.current - 1, 6),
        );
        reconnectTimer = setTimeout(open, delay);
      };
    };

    open();

    return () => {
      cancelled = true;
      if (reconnectTimer !== undefined) clearTimeout(reconnectTimer);
      es?.close();
      attemptRef.current = 0;
    };
  }, [processId, enabled, queryClient]);
}
