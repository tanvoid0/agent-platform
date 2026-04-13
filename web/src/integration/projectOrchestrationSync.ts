import type { LLMMessage } from '../core/llm/types';
import type { Task, TaskExecutionState } from './store/coreStoreTypes';

export type OrchestrationActivityInput = {
  phase: 'idle' | 'working' | 'done';
  agentsOrchestrationPaused: boolean;
  tasks: Task[];
  taskExecution: Record<string, TaskExecutionState>;
  isGeneratingAsset: boolean;
  /** Global chat: waiting for assistant reply */
  chatIsThinking: boolean;
};

/**
 * True when agents (or chat) are actively processing — distinct from phase === 'working',
 * which only means the project has started.
 */
export function hasActiveOrchestrationWork(s: OrchestrationActivityInput): boolean {
  if (s.phase !== 'working' || s.agentsOrchestrationPaused) return false;
  if (s.isGeneratingAsset || s.chatIsThinking) return true;
  if (s.tasks.some((t) => t.status === 'in_progress')) return true;

  const taskById = new Map(s.tasks.map((t) => [t.id, t]));
  for (const run of Object.values(s.taskExecution)) {
    if (run.status !== 'running' && run.status !== 'retry_queued') continue;
    const task = taskById.get(run.taskId);
    if (!task || task.status === 'done') continue;
    if (task.status === 'in_progress') return true;
  }
  return false;
}

const VISIBLE = (m: LLMMessage) => !m.metadata?.internal;

/** Last visible chat message in a thread, if any */
export function lastVisibleChatMessage(history: LLMMessage[] | undefined): LLMMessage | undefined {
  const v = (history ?? []).filter(VISIBLE);
  return v.length ? v[v.length - 1] : undefined;
}

/**
 * True when the visible thread ends with a user turn and no assistant reply yet
 * (e.g. model error, early return, or dropped handler).
 */
export function chatReplyPendingFromHistory(history: LLMMessage[] | undefined): boolean {
  const last = lastVisibleChatMessage(history);
  return last?.role === 'user';
}

/** Text to resend when {@link chatReplyPendingFromHistory} is true. */
export function pendingChatRetryText(history: LLMMessage[] | undefined): string | null {
  if (!chatReplyPendingFromHistory(history)) return null;
  const last = lastVisibleChatMessage(history);
  const c = last?.content;
  return typeof c === 'string' && c.trim() ? c.trim() : null;
}
