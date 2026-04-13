/**
 * Server chat path reachability (3D rack visual + settings). **Browser → Agent Platform only.**
 *
 * - **Server (orchestrator) chat:** `checkChatBackendHealth` → `GET /api/v1/orchestrator/ready`.
 *   The UI never contacts Ollama or the orchestrator origin directly; only the FastAPI app does.
 * - **Cloud (Gemini) chat:** This store stays idle — no background health calls to Google from here.
 */
import { create } from 'zustand';
import { getActiveChatBackendId } from '../../core/llm/llmFacade';
import { checkChatBackendHealth } from '../../core/llm/chatBackendHealth';

export type ServerChatHealthState = 'idle' | 'checking' | 'ok' | 'error';

const POLL_MS = 12_000;

let pollTimer: number | null = null;

function shouldTrackServerChatHealth(): boolean {
  return getActiveChatBackendId() === 'ollama';
}

async function probeServerChatHealth(): Promise<void> {
  if (!shouldTrackServerChatHealth()) {
    useLlmConnectivityStore.setState({
      serverChatHealth: 'idle',
      serverChatHealthDetail: '',
      llmProbeStartedAt: null,
    });
    return;
  }
  const probeStartedAt = Date.now();
  useLlmConnectivityStore.setState({
    serverChatHealth: 'checking',
    llmProbeStartedAt: probeStartedAt,
  });
  const r = await checkChatBackendHealth();
  const finishedAt = Date.now();
  if (!r.ok) {
    useLlmConnectivityStore.setState({
      serverChatHealth: 'error',
      serverChatHealthDetail: r.error ?? 'Unknown error',
      llmLastProbeFinishedAt: finishedAt,
      llmLastOkAt: null,
      llmLastLatencyMs: null,
      llmLastError: r.error ?? 'Unknown error',
      llmProbeStartedAt: null,
    });
    return;
  }
  useLlmConnectivityStore.setState({
    serverChatHealth: 'ok',
    serverChatHealthDetail: r.latencyMs != null ? `${r.latencyMs}ms` : '',
    llmLastProbeFinishedAt: finishedAt,
    llmLastOkAt: finishedAt,
    llmLastLatencyMs: r.latencyMs ?? null,
    llmLastError: null,
    llmProbeStartedAt: null,
  });
}

interface LlmConnectivityState {
  serverChatHealth: ServerChatHealthState;
  serverChatHealthDetail: string;
  llmProbeStartedAt: number | null;
  llmLastProbeFinishedAt: number | null;
  llmLastOkAt: number | null;
  llmLastLatencyMs: number | null;
  llmLastError: string | null;
  runServerChatHealthCheck: () => Promise<void>;
}

export const useLlmConnectivityStore = create<LlmConnectivityState>()(() => ({
  serverChatHealth: shouldTrackServerChatHealth() ? 'checking' : 'idle',
  serverChatHealthDetail: '',
  llmProbeStartedAt: null,
  llmLastProbeFinishedAt: null,
  llmLastOkAt: null,
  llmLastLatencyMs: null,
  llmLastError: null,
  runServerChatHealthCheck: () => probeServerChatHealth(),
}));

/** Start periodic server chat health checks when the chat path uses Agent Platform. Idempotent. */
export function ensureLlmConnectivityPolling(): void {
  if (!shouldTrackServerChatHealth()) {
    stopLlmConnectivityPolling();
    useLlmConnectivityStore.setState({
      serverChatHealth: 'idle',
      serverChatHealthDetail: '',
      llmProbeStartedAt: null,
      llmLastProbeFinishedAt: null,
      llmLastOkAt: null,
      llmLastLatencyMs: null,
      llmLastError: null,
    });
    return;
  }
  if (pollTimer !== null) return;
  void probeServerChatHealth();
  pollTimer = window.setInterval(() => void probeServerChatHealth(), POLL_MS);
}

export function stopLlmConnectivityPolling(): void {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
}
