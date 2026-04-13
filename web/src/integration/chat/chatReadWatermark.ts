import type { LLMMessage } from '../../core/llm/types';

export function visibleChatTurnCount(history: LLMMessage[] | undefined): number {
  return (history ?? []).filter((m) => !m.metadata?.internal).length;
}

/**
 * Assistant turns in the visible thread after the read watermark (`chatReadVisibleLength`).
 * Undefined watermark = user has not caught up on this thread yet (treat as 0 so new replies count).
 */
export function unreadAssistantTurnsForAgent(
  history: LLMMessage[] | undefined,
  readVisibleLength: number | undefined,
): number {
  const visible = (history ?? []).filter((m) => !m.metadata?.internal);
  const wm = readVisibleLength !== undefined ? readVisibleLength : 0;
  return visible.slice(wm).filter((m) => m.role === 'assistant').length;
}

/** Fill missing per-agent watermarks to full visible length (migration / remote load). */
export function backfillChatReadWatermarks(
  histories: Record<number, LLMMessage[]>,
  previous: Record<number, number> | undefined,
): Record<number, number> {
  const out: Record<number, number> = previous ? { ...previous } : {};
  for (const [k, hist] of Object.entries(histories)) {
    const idx = Number(k);
    if (Number.isNaN(idx)) continue;
    if (out[idx] === undefined) {
      out[idx] = visibleChatTurnCount(hist);
    }
  }
  return out;
}

export function totalUnreadAssistantTurns(
  histories: Record<number, LLMMessage[]>,
  watermarks: Record<number, number>,
): number {
  let n = 0;
  for (const [k, hist] of Object.entries(histories)) {
    const idx = Number(k);
    if (Number.isNaN(idx)) continue;
    n += unreadAssistantTurnsForAgent(hist, watermarks[idx]);
  }
  return n;
}
