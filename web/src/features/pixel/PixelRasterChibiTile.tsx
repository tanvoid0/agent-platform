import { useContext, useEffect, useLayoutEffect, useRef, useState } from "react";

import { readTshirtAccentColor } from "./pixelAccentColor";
import { PixelStripRafContext } from "./PixelStripRafContext";
import { drawCharacterSprite, frameIndexForActivity } from "./pixelCharacterDraw";
import { getPixelStripCharSheet } from "./pixelStripCharSheets";
import { PA_FRAME_H, PA_FRAME_W } from "./pixelSpriteConstants";
import { PixelChibiTile } from "./PixelChibiTile";
import type { PixelAgent } from "./pixelViewModel";
import { cn } from "@/lib/utils";

const SCALE = 2;
const CANVAS_W = PA_FRAME_W * SCALE;
const CANVAS_H = PA_FRAME_H * SCALE;

type Props = {
  agent: PixelAgent;
  /** Border / stroke color (board column). */
  statusColor: string;
  title?: string;
  pulse?: boolean;
  className?: string;
};

/**
 * MIT pixel-agents raster frame in the task strip (same sprites as the office canvas).
 * Falls back to {@link PixelChibiTile} if assets are missing or fail to load.
 */
export function PixelRasterChibiTile({ agent, statusColor, title, pulse, className }: Props) {
  const stripRaf = useContext(PixelStripRafContext);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const tintScratchRef = useRef<HTMLCanvasElement | null>(null);
  const agentRef = useRef(agent);
  agentRef.current = agent;
  const pulseRef = useRef(pulse);
  pulseRef.current = pulse;
  const statusColorRef = useRef(statusColor);
  statusColorRef.current = statusColor;
  const [sheet, setSheet] = useState<HTMLImageElement | null>(null);
  const [failed, setFailed] = useState(false);
  const tshirtRef = useRef(readTshirtAccentColor());

  const charIdx = agent.spriteCharIndex;

  useEffect(() => {
    let cancelled = false;
    setFailed(false);
    void getPixelStripCharSheet(charIdx).then((img) => {
      if (cancelled) return;
      if (img?.complete && img.naturalWidth > 0) setSheet(img);
      else setFailed(true);
    });
    return () => {
      cancelled = true;
    };
  }, [charIdx]);

  useLayoutEffect(() => {
    if (failed || !sheet?.complete || sheet.naturalWidth <= 0) return;
    if (!tintScratchRef.current) {
      const c = document.createElement("canvas");
      c.width = PA_FRAME_W;
      c.height = PA_FRAME_H;
      tintScratchRef.current = c;
    }
    const canvas = canvasRef.current;
    const tintScratch = tintScratchRef.current;
    if (!canvas || !tintScratch) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    ctx.imageSmoothingEnabled = false;

    const isDocumentHidden = () =>
      typeof document !== "undefined" && document.visibilityState === "hidden";

    const needsAnimationFrame = () => {
      const a = agentRef.current;
      const p = pulseRef.current;
      return (
        p || a.activity === "work" || a.activity === "walk" || a.activity === "attention"
      );
    };

    const paint = (tMs: number) => {
      if (isDocumentHidden()) return;

      const a = agentRef.current;
      const sc = statusColorRef.current;

      const cx = ctx;
      cx.save();
      cx.setTransform(1, 0, 0, 1, 0, 0);
      cx.clearRect(0, 0, CANVAS_W, CANVAS_H);
      cx.scale(SCALE, SCALE);
      const footX = PA_FRAME_W / 2;
      const footY = PA_FRAME_H - 1;
      const torso = a.torsoAccent ?? tshirtRef.current;
      const frame = frameIndexForActivity(a.activity, tMs, a.workHint);
      drawCharacterSprite(
        cx,
        sheet,
        frame,
        footX,
        footY,
        torso,
        sc,
        a.activity,
        tMs,
        false,
        a.workHint,
        undefined,
        false,
        tintScratch,
      );
      cx.restore();
    };

    if (stripRaf) {
      return stripRaf.subscribe({
        needsAnim: needsAnimationFrame,
        draw: paint,
      });
    }

    let raf = 0;

    const tick = (tMs: number) => {
      paint(tMs);
      const needsAnim = needsAnimationFrame();
      if (needsAnim && !isDocumentHidden()) {
        raf = requestAnimationFrame(tick);
      }
    };

    const onVisibilityChange = () => {
      if (isDocumentHidden()) return;
      if (needsAnimationFrame()) {
        cancelAnimationFrame(raf);
        raf = requestAnimationFrame(tick);
      }
    };

    document.addEventListener("visibilitychange", onVisibilityChange);

    raf = requestAnimationFrame(tick);
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      cancelAnimationFrame(raf);
    };
  }, [
    sheet,
    failed,
    pulse,
    stripRaf,
    agent.spriteCharIndex,
    agent.activity,
    agent.column,
    agent.workHint,
    agent.torsoAccent,
  ]);

  if (failed) {
    return (
      <PixelChibiTile color={statusColor} title={title} pulse={pulse} className={className} />
    );
  }

  if (!sheet) {
    return (
      <PixelChibiTile color={statusColor} title={title} pulse={pulse} className={className} />
    );
  }

  return (
    <canvas
      ref={canvasRef}
      width={CANVAS_W}
      height={CANVAS_H}
      className={cn("inline-block shrink-0 [image-rendering:pixelated]", className)}
      style={{ width: PA_FRAME_W * 0.75, height: PA_FRAME_H * 0.75 }}
      title={title}
      aria-hidden
    />
  );
}
