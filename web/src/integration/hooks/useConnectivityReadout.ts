import { useMemo } from 'react';
import { describeLlmSetup } from '../../core/llm/llmFacade';
import { useHeartbeatNow } from './useHeartbeatNow';
import { useLlmConnectivityStore } from '../store/llmConnectivityStore';
import { useProjectsApiReachabilityStore } from '../store/projectsApiReachabilityStore';

function secSince(ts: number | null, now: number): number | null {
  if (ts == null) return null;
  return Math.max(0, Math.floor((now - ts) / 1000));
}

function truncateOneLine(s: string, max: number): string {
  const t = s.replace(/\s+/g, ' ').trim();
  return t.length <= max ? t : `${t.slice(0, max - 1)}…`;
}

export type ReadoutTone = 'muted' | 'ok' | 'warn' | 'bad';

export interface ConnectivityLineReadout {
  short: string;
  detail: string;
  tone: ReadoutTone;
}

/**
 * Human-readable connectivity lines + live “heartbeat” seconds (aligned with execution HB labels).
 * Re-renders once per second while mounted so the HB counter ticks.
 */
export function useConnectivityReadout(): {
  projects: ConnectivityLineReadout | null;
  llm: ConnectivityLineReadout | null;
  now: number;
} {
  const now = useHeartbeatNow(true);

  const status = useProjectsApiReachabilityStore((s) => s.status);
  const checkStartedAt = useProjectsApiReachabilityStore((s) => s.checkStartedAt);
  const lastCheckFinishedAt = useProjectsApiReachabilityStore((s) => s.lastCheckFinishedAt);
  const lastOkAt = useProjectsApiReachabilityStore((s) => s.lastOkAt);
  const lastLatencyMs = useProjectsApiReachabilityStore((s) => s.lastLatencyMs);
  const lastError = useProjectsApiReachabilityStore((s) => s.lastError);

  const serverChatHealth = useLlmConnectivityStore((s) => s.serverChatHealth);
  const serverChatHealthDetail = useLlmConnectivityStore((s) => s.serverChatHealthDetail);
  const llmProbeStartedAt = useLlmConnectivityStore((s) => s.llmProbeStartedAt);
  const llmLastProbeFinishedAt = useLlmConnectivityStore((s) => s.llmLastProbeFinishedAt);
  const llmLastOkAt = useLlmConnectivityStore((s) => s.llmLastOkAt);
  const llmLastLatencyMs = useLlmConnectivityStore((s) => s.llmLastLatencyMs);
  const llmLastError = useLlmConnectivityStore((s) => s.llmLastError);

  const llmSetup = describeLlmSetup();

  return useMemo(() => {
    let projects: ConnectivityLineReadout | null = null;

    if (status === 'disabled') {
      projects = {
        short: 'Projects API disabled',
        detail: 'Remote project list is not configured for this build.',
        tone: 'muted',
      };
    } else if (status === 'checking') {
      const s = secSince(checkStartedAt, now);
      projects = {
        short: `Connecting…${s != null ? ` ${s}s` : ''}`,
        detail: 'Calling Agent Platform projects list…',
        tone: 'warn',
      };
    } else if (status === 'online') {
      const hb = secSince(lastOkAt, now);
      const lat = lastLatencyMs != null ? ` · ${lastLatencyMs}ms` : '';
      const hbPart = hb != null ? ` · HB ${hb}s` : '';
      projects = {
        short: `OK${hbPart}${lat}`,
        detail: `Projects API reachable. Last OK ${hb != null ? `${hb}s ago` : '—'}${lastLatencyMs != null ? ` · round-trip ${lastLatencyMs}ms` : ''}.`,
        tone: 'ok',
      };
    } else {
      const err = lastError ? truncateOneLine(lastError, 72) : 'Unreachable';
      const tryAgo = secSince(lastCheckFinishedAt, now);
      projects = {
        short: `Offline · ${truncateOneLine(err, 36)}${tryAgo != null ? ` · try ${tryAgo}s ago` : ''}`,
        detail: lastError ?? 'Agent Platform projects API could not be reached.',
        tone: 'bad',
      };
    }

    let llm: ConnectivityLineReadout | null = null;

    if (llmSetup.showServerChatHealth) {
      if (serverChatHealth === 'ok') {
        const hb = secSince(llmLastOkAt, now);
        const lat = llmLastLatencyMs != null ? ` · ${llmLastLatencyMs}ms` : serverChatHealthDetail ? ` · ${serverChatHealthDetail}` : '';
        const hbPart = hb != null ? ` · HB ${hb}s` : '';
        llm = {
          short: `OK${hbPart}${lat}`,
          detail: `Agent Platform reports the server LLM path is up. The browser never calls Ollama or the LLM directly.${llmLastLatencyMs != null ? ` Probe RTT ${llmLastLatencyMs}ms.` : ''}`,
          tone: 'ok',
        };
      } else if (serverChatHealth === 'error') {
        const err = llmLastError ? truncateOneLine(llmLastError, 72) : 'Unreachable';
        const tryAgo = secSince(llmLastProbeFinishedAt, now);
        llm = {
          short: `Offline · ${truncateOneLine(err, 36)}${tryAgo != null ? ` · try ${tryAgo}s ago` : ''}`,
          detail:
            serverChatHealthDetail ||
            llmLastError ||
            'Agent Platform could not verify the server LLM path (browser still only talked to Agent Platform).',
          tone: 'bad',
        };
      } else {
        const s = secSince(llmProbeStartedAt, now);
        llm = {
          short: `Connecting…${s != null ? ` ${s}s` : ''}`,
          detail:
            'Browser → Agent Platform only (GET /api/v1/llm/ready). The server checks the embedded LLM proxy; no direct LLM calls from the UI.',
          tone: 'warn',
        };
      }
    } else {
      llm = {
        short: 'Cloud provider',
        detail:
          'Credentials are backend-managed. Frontend no longer reads or stores AI API keys.',
        tone: 'muted',
      };
    }

    return { projects, llm, now };
  }, [
    status,
    checkStartedAt,
    lastCheckFinishedAt,
    lastOkAt,
    lastLatencyMs,
    lastError,
    serverChatHealth,
    serverChatHealthDetail,
    llmProbeStartedAt,
    llmLastProbeFinishedAt,
    llmLastOkAt,
    llmLastLatencyMs,
    llmLastError,
    llmSetup.showServerChatHealth,
    now,
  ]);
}
