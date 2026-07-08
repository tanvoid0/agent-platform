import type { PixelActivity, WorkAnimationHint } from "./pixelViewModel";
import { PA_SIT_OFFSET_PX } from "./pixelSpriteConstants";
import { PA_FRAME_H, PA_FRAME_W, PA_ROW_DOWN } from "./pixelSpriteConstants";

export function frameIndexForActivity(
  activity: PixelActivity,
  tMs: number,
  workHint?: WorkAnimationHint,
): number {
  const t = tMs / 1000;
  switch (activity) {
    case "work":
      if (workHint === "reading") {
        return 5 + (Math.floor(t * 2) % 2);
      }
      return 3 + (Math.floor(t * 3) % 2);
    case "walk":
      return [0, 1, 2, 1][Math.floor(t * 4) % 4];
    case "attention":
      return 5 + (Math.floor(t * 2) % 2);
    case "idle":
    default:
      return 1;
  }
}

function activityPhase(
  activity: PixelActivity,
  tMs: number,
  workHint?: WorkAnimationHint,
): { bob: number; sway: number; em: number } {
  const s = tMs / 1000;
  switch (activity) {
    case "work":
      if (workHint === "reading") {
        return { bob: Math.sin(s * 3) * 0.6, sway: 0, em: 1 };
      }
      return { bob: Math.sin(s * 12) * 1.5, sway: 0, em: 1 };
    case "walk":
      return { bob: Math.sin(s * 8) * 1.2, sway: Math.sin(s * 6) * 1.5, em: 0.95 };
    case "attention":
      return { bob: Math.sin(s * 4) * 0.8, sway: 0, em: Math.sin(s * 5) * 0.05 + 1 };
    case "idle":
    default:
      return { bob: Math.sin(s * 2) * 0.5, sway: 0, em: 1 };
  }
}

let scratchCanvas: HTMLCanvasElement | null = null;

function getTintScratchContext(override?: HTMLCanvasElement): CanvasRenderingContext2D {
  const el =
    override ??
    (scratchCanvas ??= (() => {
      const c = document.createElement("canvas");
      c.width = PA_FRAME_W;
      c.height = PA_FRAME_H;
      return c;
    })());
  if (override && (override.width !== PA_FRAME_W || override.height !== PA_FRAME_H)) {
    override.width = PA_FRAME_W;
    override.height = PA_FRAME_H;
  }
  const c = el.getContext("2d");
  if (!c) throw new Error("2d");
  c.imageSmoothingEnabled = false;
  return c;
}

export function drawCharacterSprite(
  ctx: CanvasRenderingContext2D,
  sheet: HTMLImageElement,
  frameIndex: number,
  footX: number,
  footY: number,
  accent: string,
  statusColor: string,
  activity: PixelActivity,
  tMs: number,
  sit: boolean,
  workHint?: WorkAnimationHint,
  /** When seated/working at a desk, keep down-facing row. */
  spriteRow: number = PA_ROW_DOWN,
  mirrorWalk = false,
  /** Per-instance scratch for parallel draws (e.g. multiple strip tiles); avoids shared tint buffer races. */
  tintScratch?: HTMLCanvasElement,
): void {
  const { bob, sway, em } = activityPhase(activity, tMs, workHint);
  const sitDy = sit ? PA_SIT_OFFSET_PX : 0;
  const row = sit ? PA_ROW_DOWN : spriteRow;
  const mirrorSide = sit ? false : mirrorWalk;
  const sx = frameIndex * PA_FRAME_W;
  const sy = row * PA_FRAME_H;

  ctx.save();
  ctx.translate(footX + sway, footY + bob + sitDy);
  ctx.scale(em, em);
  const dx = -PA_FRAME_W / 2;
  const dy = -PA_FRAME_H;

  const sc = getTintScratchContext(tintScratch);
  sc.clearRect(0, 0, PA_FRAME_W, PA_FRAME_H);
  sc.globalCompositeOperation = "source-over";
  sc.globalAlpha = 1;
  if (mirrorSide) {
    sc.save();
    sc.translate(PA_FRAME_W, 0);
    sc.scale(-1, 1);
    sc.drawImage(sheet, sx, sy, PA_FRAME_W, PA_FRAME_H, 0, 0, PA_FRAME_W, PA_FRAME_H);
    sc.restore();
  } else {
    sc.drawImage(sheet, sx, sy, PA_FRAME_W, PA_FRAME_H, 0, 0, PA_FRAME_W, PA_FRAME_H);
  }
  sc.globalCompositeOperation = "source-atop";
  sc.fillStyle = accent;
  sc.globalAlpha = 0.42;
  sc.fillRect(0, 12, PA_FRAME_W, PA_FRAME_H - 12);
  sc.globalAlpha = 1;
  sc.globalCompositeOperation = "source-over";

  const tintEl = tintScratch ?? scratchCanvas!;
  ctx.drawImage(tintEl, dx, dy);

  ctx.strokeStyle = statusColor;
  ctx.lineWidth = 1;
  ctx.strokeRect(dx - 0.5, dy - 0.5, PA_FRAME_W + 1, PA_FRAME_H + 1);

  ctx.restore();
}

export function drawFallbackFigure(
  ctx: CanvasRenderingContext2D,
  footX: number,
  footY: number,
  accent: string,
  statusColor: string,
  activity: PixelActivity,
  tMs: number,
  sit: boolean,
  workHint?: WorkAnimationHint,
): void {
  const { bob, sway, em } = activityPhase(activity, tMs, workHint);
  const sitDy = sit ? PA_SIT_OFFSET_PX : 0;
  ctx.save();
  ctx.translate(footX + sway, footY + bob + sitDy);
  ctx.scale(em, em);
  ctx.fillStyle = "#e8c4a8";
  ctx.fillRect(-5, -14, 10, 10);
  ctx.fillStyle = accent;
  ctx.fillRect(-6, -4, 12, 10);
  ctx.strokeStyle = statusColor;
  ctx.lineWidth = 1;
  ctx.strokeRect(-6.5, -4.5, 13, 11);
  ctx.restore();
}
