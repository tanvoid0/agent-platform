import { USER_COLOR } from '../../theme/brand'
import type { ProjectPhase } from '../../integration/store/coreStore'

/** Lucide icon name — mapped in `ProjectViewHeader` */
export type ProjectStatusBadgeIcon =
  | 'circle-dashed'
  | 'check-circle'
  | 'pause'
  | 'activity'
  | 'bell'
  | 'moon'

export type ProjectStatusBadgeStyle = {
  backgroundColor: string
  color: string
  borderColor: string
  pulse: boolean
  label: string
  /** Optional header icon; defaults inferred from label if omitted */
  icon: ProjectStatusBadgeIcon
  /** Shown as tooltip on the status chip */
  detail: string
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
      icon: 'circle-dashed',
      detail: 'Configure the brief and start the team when you are ready.',
    }
  }
  if (phase === 'done') {
    return {
      backgroundColor: '#22c55e',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Done',
      icon: 'check-circle',
      detail: 'Project phase complete — open output or start a clean slate.',
    }
  }
  if (phase === 'working' && agentsOrchestrationPaused) {
    return {
      backgroundColor: '#d97706',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Paused',
      icon: 'pause',
      detail: 'Orchestration is paused; resume when you want agents to continue.',
    }
  }
  if (phase === 'working' && orchestrationActive) {
    return {
      backgroundColor: USER_COLOR,
      color: 'white',
      borderColor: 'transparent',
      pulse: true,
      label: 'Working',
      icon: 'activity',
      detail: 'Agents are executing tasks or generating output.',
    }
  }
  if (phase === 'working' && activityAttentionCount > 0) {
    return {
      backgroundColor: '#c2410c',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Your turn',
      icon: 'bell',
      detail: 'Unread messages, input requests, or approvals need you in Activity.',
    }
  }
  if (phase === 'working') {
    return {
      backgroundColor: '#64748b',
      color: 'white',
      borderColor: 'transparent',
      pulse: false,
      label: 'Standby',
      icon: 'moon',
      detail: 'No agent work in flight; open Activity for unread or wait for the next step.',
    }
  }
  return {
    backgroundColor: '#f4f4f5',
    color: '#a1a1aa',
    borderColor: '#e4e4e7',
    pulse: false,
    label: String(phase),
    icon: 'circle-dashed',
    detail: 'Project status',
  }
}
