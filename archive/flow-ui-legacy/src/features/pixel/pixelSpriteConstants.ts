/** Matches pixel-agents shared/assets/constants.ts (character sheet layout). */
export const PA_FRAME_W = 16;
export const PA_FRAME_H = 32;
export const PA_FRAMES_PER_ROW = 7;
export const PA_CHAR_COUNT = 6;

/** Stable character sheet index per agent UUID (avoids repeating the same sprite by board position). */
export function spriteCharIndexFromClientUuid(clientUuid: string): number {
  let h = 0;
  for (let i = 0; i < clientUuid.length; i++) {
    h = Math.imul(31, h) + clientUuid.charCodeAt(i);
    h |= 0;
  }
  return (h >>> 0) % PA_CHAR_COUNT;
}
/** Matches pixel-agents `CHARACTER_SITTING_OFFSET_PX`. */
export const PA_SIT_OFFSET_PX = 6;

/** Character sheet rows in the 112×96 PNG (7 frames × 16px wide each row). */
export const PA_ROW_DOWN = 0;
export const PA_ROW_SIDE = 1;
export const PA_ROW_UP = 2;
