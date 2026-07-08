import { pointAlongPolyline } from "./pixelOfficePath";
import { PA_ROW_DOWN, PA_ROW_SIDE, PA_ROW_UP } from "./pixelSpriteConstants";

export interface AgentFoot {
  x: number;
  y: number;
  sit: boolean;
  /** Character sheet row (0=down, 1=side, 2=up); ignored when `sit`. */
  spriteRow?: number;
  /** Flip side row for leftward motion. */
  mirrorWalk?: boolean;
}

/** Map motion delta (screen px, +y down) to pixel-agents sheet row + mirror. */
export function spriteFacingFromDelta(dx: number, dy: number): { spriteRow: number; mirrorWalk: boolean } {
  if (Math.abs(dx) + Math.abs(dy) < 0.25) {
    return { spriteRow: PA_ROW_DOWN, mirrorWalk: false };
  }
  if (Math.abs(dx) >= Math.abs(dy)) {
    return { spriteRow: PA_ROW_SIDE, mirrorWalk: dx < 0 };
  }
  return { spriteRow: dy > 0 ? PA_ROW_DOWN : PA_ROW_UP, mirrorWalk: false };
}

/**
 * When there are more agents than desk seats, map by `slotIndex % n` repeats the same
 * seat — spread duplicates in a small ring so sprites do not stack on identical pixels.
 */
export function stackOffsetForAgentIndex(slotIndex: number, deskCount: number): { dx: number; dy: number } {
  if (deskCount <= 0) return { dx: 0, dy: 0 };
  const overflow = Math.floor(slotIndex / deskCount);
  if (overflow <= 0) return { dx: 0, dy: 0 };

  const presets: [number, number][] = [
    [-9, 0],
    [9, 0],
    [0, -7],
    [-8, -5],
    [8, -5],
    [-9, 7],
    [9, 7],
    [0, 8],
  ];
  const i = overflow - 1;
  const [ox, oy] = presets[i % presets.length]!;
  const layer = Math.floor(i / presets.length);
  return { dx: ox + layer * 14, dy: oy - layer * 2 };
}

/**
 * Office foot placement using an explicit desk seat list (full layout) or mini fallback.
 * Uses `slotIndex` (not deskSlot) so agents beyond desk count do not share one pixel.
 */
export function footForAgentWithDesks(
  agent: { slotIndex: number; activity: string },
  tMs: number,
  deskSeats: { x: number; y: number }[],
  spawn: { x: number; y: number },
  /** Optional grid paths (pixel waypoints) from `buildWalkPathsPerDesk`; same length order as `deskSeats`. */
  walkPaths?: readonly (readonly { x: number; y: number }[] | null)[] | null,
): AgentFoot {
  const n = deskSeats.length;
  const baseIdx = n > 0 ? agent.slotIndex % n : 0;
  const { dx, dy } = stackOffsetForAgentIndex(agent.slotIndex, n);
  const raw = n > 0 ? deskSeats[baseIdx]! : spawn;
  const seat = { x: raw.x + dx, y: raw.y + dy };
  const approach = { x: seat.x, y: seat.y - 12 };

  switch (agent.activity) {
    case "work":
      return { x: seat.x, y: seat.y, sit: true };
    case "walk": {
      const p = (Math.sin(tMs / 520) + 1) / 2;
      const path = walkPaths?.[baseIdx];
      if (path && path.length >= 2) {
        const pt = pointAlongPolyline(path, p);
        const dt = 0.04;
        const prevT = Math.max(0, p - dt);
        const pPrev = pointAlongPolyline(path, prevT);
        const { spriteRow, mirrorWalk } = spriteFacingFromDelta(pt.x - pPrev.x, pt.y - pPrev.y);
        return { x: pt.x + dx, y: pt.y + dy, sit: false, spriteRow, mirrorWalk };
      }
      const { spriteRow, mirrorWalk } = spriteFacingFromDelta(approach.x - spawn.x, approach.y - spawn.y);
      return {
        x: spawn.x + (approach.x - spawn.x) * p + dx,
        y: spawn.y + (approach.y - spawn.y) * p + dy,
        sit: false,
        spriteRow,
        mirrorWalk,
      };
    }
    case "attention":
      return { x: seat.x, y: seat.y, sit: false };
    case "idle":
    default:
      return { x: seat.x, y: seat.y, sit: false };
  }
}

export { PA_SIT_OFFSET_PX } from "./pixelSpriteConstants";
