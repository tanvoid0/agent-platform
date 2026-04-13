import type { DebugLogEntry } from '../../integration/store/coreStore'

/** Per request id: paired response latency, or null if still waiting. */
export type RequestPairingValue = { responseTimeMs: number } | null

/**
 * For each request entry id: matching response latency (request timestamp → response
 * timestamp), or null if no response yet. FIFO per agent + taskId, chronological order.
 */
export function buildRequestPairingMap(
  debugLog: readonly DebugLogEntry[]
): Map<string, RequestPairingValue> {
  const result = new Map<string, RequestPairingValue>()
  const pending = new Map<string, { id: string; timestamp: number }[]>()

  const queueKey = (e: { agentIndex: number; taskId?: string }) =>
    `${e.agentIndex}\0${e.taskId ?? ''}`

  for (const entry of debugLog) {
    if (entry.phase === 'request') {
      const k = queueKey(entry)
      const q = pending.get(k) ?? []
      q.push({ id: entry.id, timestamp: entry.timestamp })
      pending.set(k, q)
      result.set(entry.id, null)
    } else {
      const k = queueKey(entry)
      const q = pending.get(k)
      if (q && q.length > 0) {
        const req = q.shift()!
        if (q.length === 0) pending.delete(k)
        else pending.set(k, q)
        const responseTimeMs = Math.max(0, entry.timestamp - req.timestamp)
        result.set(req.id, { responseTimeMs })
      }
    }
  }

  return result
}

/** Compact duration for LLM round-trip (clipboard + list row). */
export function formatResponseDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '—'
  if (ms < 1000) return `${Math.round(ms)}ms`
  const s = ms / 1000
  return s < 10 ? `${s.toFixed(1)}s` : `${Math.round(s)}s`
}
