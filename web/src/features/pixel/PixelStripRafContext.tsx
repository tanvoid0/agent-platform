import {
  createContext,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
} from "react";

export type PixelStripSubscriber = {
  needsAnim: () => boolean;
  draw: (tMs: number) => void;
};

type Ctx = {
  subscribe: (sub: PixelStripSubscriber) => () => void;
};

export const PixelStripRafContext = createContext<Ctx | null>(null);

/**
 * Single RAF loop for all raster strip tiles (avoids N timers when many tasks are visible).
 */
export function PixelStripRafProvider({ children }: { children: ReactNode }) {
  const subsRef = useRef(new Set<PixelStripSubscriber>());
  const rafRef = useRef(0);

  const runFrame = useCallback((tMs: number) => {
    if (typeof document !== "undefined" && document.visibilityState === "hidden") {
      rafRef.current = 0;
      return;
    }
    for (const sub of subsRef.current) {
      if (sub.needsAnim()) sub.draw(tMs);
    }
    const any = [...subsRef.current].some((s) => s.needsAnim());
    if (any && document.visibilityState !== "hidden") {
      rafRef.current = requestAnimationFrame(runFrame);
    } else {
      rafRef.current = 0;
    }
  }, []);

  const subscribe = useCallback(
    (sub: PixelStripSubscriber) => {
      subsRef.current.add(sub);
      sub.draw(performance.now());
      const any = [...subsRef.current].some((s) => s.needsAnim());
      if (any && rafRef.current === 0) {
        rafRef.current = requestAnimationFrame(runFrame);
      }
      return () => {
        subsRef.current.delete(sub);
        const anyLeft = [...subsRef.current].some((s) => s.needsAnim());
        if (!anyLeft && rafRef.current) {
          cancelAnimationFrame(rafRef.current);
          rafRef.current = 0;
        }
      };
    },
    [runFrame],
  );

  useEffect(() => {
    const onVis = () => {
      if (document.visibilityState === "hidden") {
        if (rafRef.current) {
          cancelAnimationFrame(rafRef.current);
          rafRef.current = 0;
        }
        return;
      }
      const t = performance.now();
      for (const sub of subsRef.current) {
        sub.draw(t);
      }
      const any = [...subsRef.current].some((s) => s.needsAnim());
      if (any && rafRef.current === 0) {
        rafRef.current = requestAnimationFrame(runFrame);
      }
    };
    document.addEventListener("visibilitychange", onVis);
    return () => document.removeEventListener("visibilitychange", onVis);
  }, [runFrame]);

  const value = useMemo(() => ({ subscribe }), [subscribe]);
  return (
    <PixelStripRafContext.Provider value={value}>{children}</PixelStripRafContext.Provider>
  );
}
