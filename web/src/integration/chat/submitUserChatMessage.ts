import type { LLMMessage } from '../../core/llm/types';
import { visibleChatTurnCount } from './chatReadWatermark';
import { useCoreStore } from '../store/coreStore';
import { resetNpcSpeechPulseUi, useUiStore } from '../store/uiStore';

export type UserChatHandler = (npcIndex: number, text: string) => Promise<string | null>;

export type SubmitUserChatMessageOptions = {
  /** Remove the last visible user message before appending (avoids duplicate lines on retry). */
  replaceLastUser?: boolean;
};

function stripLastVisibleUserMessage(hist: LLMMessage[]) {
  const next = [...hist];
  for (let i = next.length - 1; i >= 0; i--) {
    const m = next[i];
    if (m.metadata?.internal) continue;
    if (m.role === 'user') {
      next.splice(i, 1);
      return next;
    }
    return hist;
  }
  return hist;
}

/**
 * Appends the user turn, runs the simulation/LLM pipeline, and manages thinking UI flags.
 * Called from the renderer (`SceneManager`); keeps chat orchestration out of React.
 */
export async function submitUserChatMessage(
  handler: UserChatHandler | null,
  npcIndex: number,
  text: string,
  options?: SubmitUserChatMessageOptions
): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;
  if (useUiStore.getState().isThinking) return;

  useCoreStore.setState((s) => {
    let base = [...(s.agentHistories[npcIndex] || [])];
    if (options?.replaceLastUser) {
      base = stripLastVisibleUserMessage(base);
    }
    const nextHist = [...base, { role: 'user' as const, content: trimmed }];
    const vis = visibleChatTurnCount(nextHist);
    return {
      agentHistories: {
        ...s.agentHistories,
        [npcIndex]: nextHist,
      },
      chatReadVisibleLength: {
        ...s.chatReadVisibleLength,
        [npcIndex]: vis,
      },
    };
  });
  resetNpcSpeechPulseUi();
  useUiStore.setState({ isThinking: true, isTyping: false });
  try {
    if (handler) await handler(npcIndex, trimmed);
    useUiStore.setState({ isThinking: false });
    useUiStore.getState().triggerNpcSpeechPulse();
  } catch (err) {
    console.error('[submitUserChatMessage]', err);
    useUiStore.setState({ isThinking: false });
  }
}
