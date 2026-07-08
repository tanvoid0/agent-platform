import { useEffect, useLayoutEffect, useRef } from "react";

import { TASK_STATUS_COLORS } from "../../lib/taskStatusVisual";
import type { PixelSceneState } from "./pixelViewModel";
import { loadDeskFrontImage, loadPixelAgentImages } from "./loadPixelAgentImages";
import { readTshirtAccentColor } from "./pixelAccentColor";
import {
  drawCharacterSprite,
  drawFallbackFigure,
  frameIndexForActivity,
} from "./pixelCharacterDraw";
import { footForAgent, OFFICE_H, OFFICE_W, OFFICE_DESKS, PA_DESK_H, PA_DESK_W } from "./pixelOfficeLayout";
import { PA_ROW_DOWN } from "./pixelSpriteConstants";

const TILE = 16;
const MAX_VISIBLE = 32;

function drawWoodFloor(ctx: CanvasRenderingContext2D, w: number, h: number) {
  const a = "#c4a574";
  const b = "#b8956a";
  for (let y = 0; y < h; y += TILE) {
    for (let x = 0; x < w; x += TILE) {
      ctx.fillStyle = (x / TILE + y / TILE) % 2 === 0 ? a : b;
      ctx.fillRect(x, y, TILE, TILE);
    }
  }
}

function drawDesks(ctx: CanvasRenderingContext2D, deskImg: HTMLImageElement | null) {
  if (!deskImg?.complete || deskImg.naturalWidth <= 0) {
    for (const d of OFFICE_DESKS) {
      ctx.fillStyle = "rgba(120, 80, 40, 0.85)";
      ctx.fillRect(d.deskX, d.deskY, PA_DESK_W, PA_DESK_H);
      ctx.strokeStyle = "rgba(60, 40, 20, 0.9)";
      ctx.strokeRect(d.deskX, d.deskY, PA_DESK_W, PA_DESK_H);
    }
    return;
  }
  for (const d of OFFICE_DESKS) {
    ctx.drawImage(deskImg, d.deskX, d.deskY);
  }
}

function drawScene(
  ctx: CanvasRenderingContext2D,
  scene: PixelSceneState,
  sheets: HTMLImageElement[] | null,
  deskImg: HTMLImageElement | null,
  accent: string,
  tMs: number,
) {
  ctx.imageSmoothingEnabled = false;
  drawWoodFloor(ctx, OFFICE_W, OFFICE_H);
  drawDesks(ctx, deskImg);

  const agents = scene.agents.slice(0, MAX_VISIBLE);
  const withFoot = agents.map((a) => ({
    agent: a,
    foot: footForAgent(a, tMs),
  }));
  withFoot.sort((p, q) => p.foot.y - q.foot.y);

  for (const { agent: a, foot } of withFoot) {
    const statusColor = TASK_STATUS_COLORS[a.column];
    const charIdx = a.spriteCharIndex;
    const frame = frameIndexForActivity(a.activity, tMs, a.workHint);
    const sheet = sheets?.[charIdx];
    const torso = a.torsoAccent ?? accent;
    if (sheet && sheet.complete && sheet.naturalWidth > 0) {
      drawCharacterSprite(
        ctx,
        sheet,
        frame,
        foot.x,
        foot.y,
        torso,
        statusColor,
        a.activity,
        tMs,
        foot.sit,
        a.workHint,
        foot.spriteRow ?? PA_ROW_DOWN,
        foot.mirrorWalk ?? false,
      );
    } else {
      drawFallbackFigure(ctx, foot.x, foot.y, torso, statusColor, a.activity, tMs, foot.sit, a.workHint);
    }
  }

  if (agents.length === 0) {
    ctx.fillStyle = "rgba(100, 116, 139, 0.95)";
    ctx.font = "10px ui-monospace, monospace";
    ctx.textAlign = "center";
    ctx.fillText("No agents", OFFICE_W / 2, OFFICE_H / 2 + 3);
  }
}

type Props = {
  scene: PixelSceneState;
  className?: string;
};

export default function PixelOfficeCanvas({ scene, className }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sceneRef = useRef(scene);
  sceneRef.current = scene;
  const tshirtRef = useRef(readTshirtAccentColor());
  const sheetsRef = useRef<HTMLImageElement[] | null>(null);
  const deskRef = useRef<HTMLImageElement | null>(null);

  useLayoutEffect(() => {
    const sync = () => {
      tshirtRef.current = readTshirtAccentColor();
    };
    sync();
    const mo = new MutationObserver(sync);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "style"] });
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    mq.addEventListener("change", sync);
    return () => {
      mo.disconnect();
      mq.removeEventListener("change", sync);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([loadPixelAgentImages(), loadDeskFrontImage()])
      .then(([chars, desk]) => {
        if (!cancelled) {
          sheetsRef.current = chars;
          deskRef.current = desk;
        }
      })
      .catch(() => {
        if (!cancelled) {
          sheetsRef.current = null;
          deskRef.current = null;
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    let raf = 0;
    let cancelled = false;
    const parent = canvas.parentElement;
    const lastDim = { bufW: -1, bufH: -1, cssW: "", cssH: "" };

    const layout = () => {
      const rect = parent?.getBoundingClientRect();
      const maxW = rect && rect.width > 0 ? rect.width : OFFICE_W;
      const scale = Math.max(1, Math.floor(Math.min(maxW / OFFICE_W, 4)));
      const dpr = window.devicePixelRatio || 1;
      const integerScale = Math.max(1, Math.floor(dpr)) * scale;

      const cssW = `${Math.min(OFFICE_W * scale, maxW)}px`;
      const cssH = `${OFFICE_H * scale}px`;
      const bufW = Math.floor(Math.min(OFFICE_W * integerScale, maxW * dpr));
      const bufH = Math.floor(OFFICE_H * integerScale);

      if (bufW !== lastDim.bufW || bufH !== lastDim.bufH || cssW !== lastDim.cssW || cssH !== lastDim.cssH) {
        canvas.style.width = cssW;
        canvas.style.height = cssH;
        canvas.width = bufW;
        canvas.height = bufH;
        lastDim.bufW = bufW;
        lastDim.bufH = bufH;
        lastDim.cssW = cssW;
        lastDim.cssH = cssH;
      }

      ctx.setTransform(integerScale, 0, 0, integerScale, 0, 0);
    };

    const loop = (t: number) => {
      if (cancelled) return;
      if (document.visibilityState === "hidden") {
        raf = requestAnimationFrame(loop);
        return;
      }
      layout();
      drawScene(
        ctx,
        sceneRef.current,
        sheetsRef.current,
        deskRef.current,
        tshirtRef.current,
        t,
      );
      raf = requestAnimationFrame(loop);
    };

    raf = requestAnimationFrame(loop);

    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
    };
  }, []);

  return (
    <canvas
      ref={canvasRef}
      className={className}
      aria-hidden
      style={{ imageRendering: "pixelated", display: "block", maxWidth: "100%" }}
    />
  );
}
