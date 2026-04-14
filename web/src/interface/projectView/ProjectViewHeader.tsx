import {
  Activity,
  Bell,
  CheckCircle2,
  CircleDashed,
  Eraser,
  FolderOpen,
  Moon,
  Pause,
} from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import { InfoTooltip } from '../components/InfoTooltip'
import type { ProjectStatusBadgeIcon, ProjectStatusBadgeStyle } from './projectStatusBadge'

const BADGE_ICONS: Record<
  ProjectStatusBadgeIcon,
  React.ComponentType<{ className?: string; strokeWidth?: number; size?: number }>
> = {
  'circle-dashed': CircleDashed,
  'check-circle': CheckCircle2,
  pause: Pause,
  activity: Activity,
  bell: Bell,
  moon: Moon,
}

export const ProjectViewHeader: React.FC<{
  badge: ProjectStatusBadgeStyle
  /** Opens the same reset modal as Overview → “New or reset…” — full local wipe (+ server sync when configured). */
  onRequestCleanSlate?: () => void
  /** When set (e.g. phase done), shows a primary control to open the full project output page. */
  onViewProjectOutput?: () => void
}> = ({ badge, onRequestCleanSlate, onViewProjectOutput }) => {
  const BadgeGlyph = BADGE_ICONS[badge.icon]
  return (
  <div className="mb-6">
    <div className="flex flex-wrap items-center justify-between mb-2 gap-3">
      <h2 className="text-xl font-black text-darkDelegation leading-tight">Project Info</h2>
      <div className="flex flex-wrap items-center justify-end gap-2 shrink-0">
        {onViewProjectOutput ? (
          <Button
            type="button"
            variant="default"
            size="sm"
            onClick={onViewProjectOutput}
            className="h-8 gap-1.5 bg-amber-400 px-2.5 text-[9px] font-black uppercase tracking-widest text-black shadow-sm hover:bg-amber-300"
          >
            <FolderOpen className="size-3.5 shrink-0" strokeWidth={2.5} aria-hidden />
            View project output
          </Button>
        ) : null}
        {onRequestCleanSlate ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={onRequestCleanSlate}
            className="h-8 gap-1.5 border-zinc-200 bg-white px-2.5 text-[9px] font-black uppercase tracking-widest text-zinc-500 shadow-sm hover:border-red-200 hover:bg-red-50/80 hover:text-red-700"
            title="Clear tasks, chats, execution state, and logs for this project (optionally syncs empty payload to server)"
          >
            <Eraser className="size-3.5 shrink-0 opacity-70" strokeWidth={2.25} aria-hidden />
            Clean slate
          </Button>
        ) : null}
        <InfoTooltip text={badge.detail} maxWidth={260}>
          <div
            className="px-2.5 py-1 rounded-xl text-[9px] font-black uppercase tracking-widest flex items-center gap-1.5 transition-colors border border-transparent cursor-default"
            style={{
              backgroundColor: badge.backgroundColor,
              color: badge.color,
              borderColor: badge.borderColor,
            }}
          >
            <BadgeGlyph className="size-3.5 shrink-0 opacity-95" strokeWidth={2.25} aria-hidden />
            <div
              className={`w-1.5 h-1.5 rounded-full ${
                badge.pulse ? 'bg-white animate-pulse' : 'bg-white opacity-40'
              }`}
            />
            {badge.label}
          </div>
        </InfoTooltip>
      </div>
    </div>
  </div>
  )
}
