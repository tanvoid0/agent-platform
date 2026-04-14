/**
 * Chat-stack readiness from the browser’s perspective: **only** `GET` on Agent Platform
 * (`/api/v1/llm/ready`). The UI never probes Ollama or any LLM host directly — the
 * backend checks the embedded LLM proxy via `/v1/health/readiness` (fast), not a full
 * `/v1/models` fan-out (can exceed this request’s timeout).
 */
import { agentPlatformAuthHeaders, apiUrl } from '../../api/client';

export interface ChatBackendHealthResult {
  ok: boolean;
  latencyMs?: number;
  error?: string;
}

const TIMEOUT_MS = 4500;

export async function checkChatBackendHealth(): Promise<ChatBackendHealthResult> {
  const url = apiUrl('/api/v1/llm/ready');
  const ctrl = new AbortController();
  const started = performance.now();
  const timer = window.setTimeout(() => ctrl.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(url, {
      method: 'GET',
      signal: ctrl.signal,
      headers: { Accept: 'application/json', ...agentPlatformAuthHeaders() },
    });
    window.clearTimeout(timer);
    const latencyMs = Math.round(performance.now() - started);
    if (!res.ok) {
      let detail: string | null = null;
      const requestId = res.headers.get('x-request-id');
      try {
        const j = (await res.json()) as { detail?: unknown };
        if (typeof j.detail === 'string') detail = j.detail;
        else if (Array.isArray(j.detail)) detail = j.detail.map(String).join('; ');
      } catch {
        try {
          const raw = (await res.text()).trim();
          if (raw) detail = raw.slice(0, 400);
        } catch {
          /* ignore */
        }
      }
      const parts = [`HTTP ${res.status}`];
      if (detail) parts.push(detail);
      if (requestId) parts.push(`request_id=${requestId}`);
      return { ok: false, error: parts.join(' | ') };
    }
    return { ok: true, latencyMs };
  } catch (e) {
    window.clearTimeout(timer);
    const name = e instanceof DOMException ? e.name : e instanceof Error ? e.name : '';
    const msg = e instanceof Error ? e.message : String(e);
    if (name === 'AbortError' || /aborted/i.test(msg)) {
      return { ok: false, error: `Timeout (${TIMEOUT_MS}ms)` };
    }
    return { ok: false, error: msg };
  }
}
