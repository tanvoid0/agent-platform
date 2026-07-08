import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { TASK_STATUS_COLORS } from "../../lib/taskStatusVisual";
import type { PixelLayoutJson } from "./layoutTypes";
import { buildFurnitureSpecOverridesFromManifests } from "./furnitureManifest";
import { resolveFurnitureType, type ResolvedFurnitureSprite } from "./furnitureResolve";
import { loadPixelAgentImages } from "./loadPixelAgentImages";
import { readTshirtAccentColor } from "./pixelAccentColor";
import {
  drawCharacterSprite,
  drawFallbackFigure,
  frameIndexForActivity,
} from "./pixelCharacterDraw";
import { deskIndexForPcFront, extractDeskSeatPixels, TILE_PX } from "./layoutDeskSeats";
import { buildWalkPathsPerDesk, resolveWalkableSpawnPixels } from "./pixelOfficePath";
import { footForAgentWithDesks } from "./pixelOfficeFoot";
import type { PixelSceneState } from "./pixelViewModel";
import { PA_ROW_DOWN } from "./pixelSpriteConstants";
import { drawTexturedTileLayer } from "./pixelTileSprites";

const MAX_VISIBLE = 32;

function drawTileLayer(
  ctx: CanvasRenderingContext2D,
  layout: PixelLayoutJson,
  cache: Map<string, HTMLImageElement | null>,
) {
  drawTexturedTileLayer(ctx, layout, cache);
}

function drawFurnitureLayer(
  ctx: CanvasRenderingContext2D,
  layout: PixelLayoutJson,
  cache: Map<string, HTMLImageElement | null>,
  scene: PixelSceneState,
  tMs: number,
  furnitureSpecs: Map<string, ResolvedFurnitureSprite> | null,
) {
  const deskCount = extractDeskSeatPixels(layout).length;
  const items = [...layout.furniture]
    .map((f, i) => ({ f, i, spec: resolveFurnitureType(f.type, furnitureSpecs) }))
    .filter((x): x is typeof x & { spec: NonNullable<typeof x.spec> } => x.spec !== null)
    .sort((a, b) => a.f.row + a.f.col / 1000 - (b.f.row + b.f.col / 1000));

  for (const { f, spec } of items) {
    let key = `${spec.dir}/${spec.file}`;
    if (f.type === "PC_FRONT_OFF" && deskCount > 0) {
      const di = deskIndexForPcFront(layout, f);
      if (di != null) {
        const agent = scene.agents.find((a) => a.slotIndex % deskCount === di);
        if (agent?.activity === "work") {
          const fr = 1 + (Math.floor(tMs / 280) % 3);
          key = `PC/PC_FRONT_ON_${fr}.png`;
        }
      }
    }
    let img = cache.get(key);
    if (!img?.complete || img.naturalWidth <= 0) {
      const fb = `${spec.dir}/${spec.file}`;
      if (key !== fb) img = cache.get(fb);
    }
    if (!img?.complete || img.naturalWidth <= 0) continue;
    const x = f.col * TILE_PX;
    const y = f.row * TILE_PX;
    const { w, h, mirrorX } = spec;
    ctx.save();
    ctx.imageSmoothingEnabled = false;
    if (mirrorX) {
      ctx.translate(x + w, y);
      ctx.scale(-1, 1);
      ctx.drawImage(img, 0, 0, w, h);
    } else {
      ctx.drawImage(img, x, y, w, h);
    }
    ctx.restore();
  }
}

function drawAgentsLayer(
  ctx: CanvasRenderingContext2D,
  layout: PixelLayoutJson,
  scene: PixelSceneState,
  sheets: HTMLImageElement[] | null,
  accent: string,
  tMs: number,
  deskSeats: { x: number; y: number }[],
  spawn: { x: number; y: number },
  walkPaths: readonly (readonly { x: number; y: number }[] | null)[] | null,
) {
  const agents = scene.agents.slice(0, MAX_VISIBLE);
  const withFoot = agents.map((a) => ({
    agent: a,
    foot: footForAgentWithDesks(a, tMs, deskSeats, spawn, walkPaths),
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
    const cx = (layout.cols * TILE_PX) / 2;
    const cy = (layout.rows * TILE_PX) / 2 + 3;
    ctx.fillText("No agents", cx, cy);
  }
}

function drawFullScene(
  ctx: CanvasRenderingContext2D,
  layout: PixelLayoutJson,
  scene: PixelSceneState,
  sheets: HTMLImageElement[] | null,
  cache: Map<string, HTMLImageElement | null>,
  accent: string,
  tMs: number,
  walkPaths: readonly (readonly { x: number; y: number }[] | null)[] | null,
  furnitureSpecs: Map<string, ResolvedFurnitureSprite> | null,
) {
  ctx.imageSmoothingEnabled = false;

  drawTileLayer(ctx, layout, cache);
  drawFurnitureLayer(ctx, layout, cache, scene, tMs, furnitureSpecs);

  const deskSeats = extractDeskSeatPixels(layout);
  const spawn = resolveWalkableSpawnPixels(layout);
  drawAgentsLayer(ctx, layout, scene, sheets, accent, tMs, deskSeats, spawn, walkPaths);
}

type Props = {
  scene: PixelSceneState;
  className?: string;
};

export default function PixelOfficeFullCanvas({ scene, className }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sceneRef = useRef(scene);
  sceneRef.current = scene;
  const tshirtRef = useRef(readTshirtAccentColor());
  const sheetsRef = useRef<HTMLImageElement[] | null>(null);
  const layoutRef = useRef<PixelLayoutJson | null>(null);
  const walkPathsRef = useRef<readonly (readonly { x: number; y: number }[] | null)[] | null>(null);
  const furnitureSpecsRef = useRef<Map<string, ResolvedFurnitureSprite> | null>(null);
  const furnitureCacheRef = useRef<Map<string, HTMLImageElement | null>>(new Map());
  const loadErrorRef = useRef(false);
  const [ready, setReady] = useState(false);

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
    const base = import.meta.env.BASE_URL;

    void (async () => {
      try {
        loadErrorRef.current = false;
        const [layoutRes, chars] = await Promise.all([
          fetch(`${base}pixel-agents/default-layout-1.json`),
          loadPixelAgentImages(),
        ]);
        if (cancelled) return;
        if (!layoutRes.ok) throw new Error(`layout ${layoutRes.status}`);
        const layout = (await layoutRes.json()) as PixelLayoutJson;
        layoutRef.current = layout;
        const deskSeats = extractDeskSeatPixels(layout);
        const spawn = resolveWalkableSpawnPixels(layout);
        walkPathsRef.current = buildWalkPathsPerDesk({ layout, deskSeats, spawn });
        sheetsRef.current = chars;

        const furnitureSpecs = await buildFurnitureSpecOverridesFromManifests(base, layout);
        furnitureSpecsRef.current = furnitureSpecs;

        const urls = new Set<string>();
        for (const f of layout.furniture) {
          const spec = resolveFurnitureType(f.type, furnitureSpecs);
          if (spec) urls.add(`${base}pixel-agents/assets/furniture/${spec.dir}/${spec.file}`);
        }
        for (let i = 1; i <= 3; i++) {
          urls.add(`${base}pixel-agents/assets/furniture/PC/PC_FRONT_ON_${i}.png`);
        }
        for (let i = 0; i <= 8; i++) {
          urls.add(`${base}pixel-agents/assets/floors/floor_${i}.png`);
        }
        urls.add(`${base}pixel-agents/assets/walls/wall_0.png`);

        const cache = furnitureCacheRef.current;
        cache.clear();
        const assetsPrefix = `${base}pixel-agents/assets/`;
        await Promise.all(
          [...urls].map(
            (url) =>
              new Promise<void>((resolve) => {
                const img = new Image();
                img.decoding = "async";
                const setCache = (value: HTMLImageElement | null) => {
                  const rel = url.startsWith(assetsPrefix) ? url.slice(assetsPrefix.length) : url;
                  cache.set(rel, value);
                };
                img.onload = () => {
                  setCache(img);
                  resolve();
                };
                img.onerror = () => {
                  setCache(null);
                  resolve();
                };
                img.src = url;
              }),
          ),
        );
        if (!cancelled) setReady(true);
      } catch {
        if (!cancelled) {
          layoutRef.current = null;
          walkPathsRef.current = null;
          furnitureSpecsRef.current = null;
          loadErrorRef.current = true;
          setReady(false);
        }
      }
    })();

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

    const layoutFn = () => {
      const layout = layoutRef.current;
      const lw = layout ? layout.cols * TILE_PX : 176;
      const lh = layout ? layout.rows * TILE_PX : 128;
      const rect = parent?.getBoundingClientRect();
      const maxW = rect && rect.width > 0 ? rect.width : lw;
      const scale = Math.max(1, Math.floor(Math.min(maxW / lw, 3)));
      const dpr = window.devicePixelRatio || 1;
      const integerScale = Math.max(1, Math.floor(dpr)) * scale;

      const cssW = `${Math.min(lw * scale, maxW)}px`;
      const cssH = `${lh * scale}px`;
      const bufW = Math.floor(Math.min(lw * integerScale, maxW * dpr));
      const bufH = Math.floor(lh * integerScale);

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
      layoutFn();
      const layout = layoutRef.current;
      if (layout && ready) {
        drawFullScene(
          ctx,
          layout,
          sceneRef.current,
          sheetsRef.current,
          furnitureCacheRef.current,
          tshirtRef.current,
          t,
          walkPathsRef.current,
          furnitureSpecsRef.current,
        );
      } else {
        ctx.fillStyle = "rgba(148, 163, 184, 0.25)";
        const lw = layout?.cols ? layout.cols * TILE_PX : 176;
        const lh = layout?.rows ? layout.rows * TILE_PX : 128;
        ctx.fillRect(0, 0, lw, lh);
        ctx.fillStyle = "rgba(100, 116, 139, 0.9)";
        ctx.font = "10px ui-monospace, monospace";
        ctx.textAlign = "center";
        const msg = loadErrorRef.current ? "Office layout unavailable" : "Loading office…";
        ctx.fillText(msg, lw / 2, lh / 2);
      }
      raf = requestAnimationFrame(loop);
    };

    raf = requestAnimationFrame(loop);

    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
    };
  }, [ready]);

  return (
    <canvas
      ref={canvasRef}
      className={className}
      aria-hidden
      style={{ imageRendering: "pixelated", display: "block", maxWidth: "100%" }}
    />
  );
}
