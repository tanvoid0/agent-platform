import { useMemo } from 'react'
import { useCoreStore } from '../../integration/store/coreStore'
import { computeProjectStatusBadge, type ProjectStatusBadgeStyle } from './projectStatusBadge'
import type { InputRequestItem } from './buildInputRequestList'
import { useActivityAttentionCount } from './useActivityAttentionCount'

export type ProjectStatusBadgeBundle = {
  badge: ProjectStatusBadgeStyle
  activityAttentionCount: number
  inputRequests: InputRequestItem[]
  orchestrationActive: boolean
}

/**
 * Single source for the Project Info status chip and any global chrome (header) — one
 * `useActivityAttentionCount` subscription per consumer.
 */
export function useProjectStatusBadge(): ProjectStatusBadgeBundle {
  const phase = useCoreStore((s) => s.phase)
  const agentsOrchestrationPaused = useCoreStore((s) => s.agentsOrchestrationPaused)
  const { activityAttentionCount, inputRequests, orchestrationActive } = useActivityAttentionCount()

  const badge = useMemo(
    () => computeProjectStatusBadge(phase, agentsOrchestrationPaused, orchestrationActive, activityAttentionCount),
    [phase, agentsOrchestrationPaused, orchestrationActive, activityAttentionCount],
  )

  return { badge, activityAttentionCount, inputRequests, orchestrationActive }
}
