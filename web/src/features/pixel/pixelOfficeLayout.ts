import { footForAgentWithDesks, type AgentFoot } from "./pixelOfficeFoot";
import { PA_SIT_OFFSET_PX } from "./pixelSpriteConstants";

export { PA_SIT_OFFSET_PX };

/** Pixel-agents DESK_FRONT sprite size. */
export const PA_DESK_W = 48;
export const PA_DESK_H = 32;

export interface DeskSlot {
  deskX: number;
  deskY: number;
  /** Foot anchor for the character (sprite bottom-center). */
  seatX: number;
  seatY: number;
}

/** Four desks in a 2×2 arrangement (presentation-only; not authoritative). */
export const OFFICE_DESKS: DeskSlot[] = [
  { deskX: 8, deskY: 16, seatX: 32, seatY: 56 },
  { deskX: 96, deskY: 16, seatX: 120, seatY: 56 },
  { deskX: 8, deskY: 72, seatX: 32, seatY: 112 },
  { deskX: 96, deskY: 72, seatX: 120, seatY: 112 },
];

/** Hallway spawn for walk animation (toward assigned desk). */
export const OFFICE_SPAWN = { x: 88, y: 104 };

export const OFFICE_W = 176;
export const OFFICE_H = 128;

export const DESK_COUNT = OFFICE_DESKS.length;

/** Legacy modulo for `PixelAgent.deskSlot` (cosmetic); foot placement uses `slotIndex` + stagger. */
export const DESK_SLOT_MOD = DESK_COUNT;

export type { AgentFoot } from "./pixelOfficeFoot";

export interface AgentForFoot {
  slotIndex: number;
  activity: string;
}

const MINI_SEATS = OFFICE_DESKS.map((d) => ({ x: d.seatX, y: d.seatY }));

/**
 * Cosmetic foot position by board order (`slotIndex`); motion is purely visual.
 */
export function footForAgent(agent: AgentForFoot, tMs: number): AgentFoot {
  return footForAgentWithDesks(agent, tMs, MINI_SEATS, OFFICE_SPAWN);
}
