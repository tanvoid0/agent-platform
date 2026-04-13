import { USER_COLOR } from '../../theme/brand'
import type { ProjectPhase } from '../../integration/store/coreStore'

export type ProjectStatusBadgeStyle = {
  backgroundColor: string
  color: string
  borderColor: string
  pulse: boolean
  label: string
}

export function computeProjectStatusBadge(
  phase: ProjectPhase,
  agentsOrchestrationPaused: boolean,
  orchestrationActive: boolean,
  activityAttentionCount: number,
): ProjectStatusBadgeStyle {
  if (phase === 'idle') {
    return {
      backgroundColor: '#f4f4f5',
      color: '#a1a1aa',
      borderColor: '#e4e4e7',
      pulse: false,
      label: 'Ready to Start',
    }
  }
  if (phase === 'done') {
    return {
      backgroundColor: '#22c55e',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Done',
    }
  }
  if (phase === 'working' && agentsOrchestrationPaused) {
    return {
      backgroundColor: '#d97706',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Paused',
    }
  }
  if (phase === 'working' && orchestrationActive) {
    return {
      backgroundColor: USER_COLOR,
      color: 'white',
      borderColor: 'transparent',
      pulse: true,
      label: 'Working',
    }
  }
  if (phase === 'working' && activityAttentionCount > 0) {
    return {
      backgroundColor: '#c2410c',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Your turn',
    }
  }
  if (phase === 'working') {
    return {
      backgroundColor: '#64748b',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Standby',
    }
  }
  return {
    backgroundColor: '#f4f4f5',
    color: '#a1a1aa',
    borderColor: '#e4e4e7',
    pulse: false,
    label: String(phase),
  }
}
