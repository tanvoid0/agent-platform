import { Activity, Info, PanelLeftOpen, PanelRightClose, Users } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import type { ProjectSideTab } from './ProjectSideTabs'

const RAIL_W = 'w-[52px]'

export const ProjectSideIconRail: React.FC<{
  sideTab: ProjectSideTab
  onSideTab: (tab: ProjectSideTab) => void
  expanded: boolean
  onExpandedChange: (next: boolean) => void
  activityAttentionCount: number
  agentsAttentionCount: number
  /** When true, no tab looks selected (e.g. chat is open). */
  muteTabSelection?: boolean
}> = ({
  sideTab,
  onSideTab,
  expanded,
  onExpandedChange,
  activityAttentionCount,
  agentsAttentionCount,
  muteTabSelection = false,
}) => {
  const selectTab = (tab: ProjectSideTab) => {
    if (!expanded) {
      onExpandedChange(true)
    }
    onSideTab(tab)
  }

  const tabBtn = (
    tab: ProjectSideTab,
    label: string,
    icon: React.ReactNode,
    badge?: number,
  ) => (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      title={label}
      aria-label={
        tab === 'activity' && badge && badge > 0
          ? `${label}, ${badge} ${badge === 1 ? 'item needs' : 'items need'} your attention`
          : label
      }
      aria-pressed={!muteTabSelection && sideTab === tab}
      onClick={() => selectTab(tab)}
      className={`relative size-10 shrink-0 rounded-lg ${
        sideTab === tab
          ? 'bg-zinc-100 text-darkDelegation'
          : 'text-zinc-400 hover:bg-zinc-50 hover:text-zinc-600'
      }`}
    >
      {icon}
      {badge !== undefined && badge > 0 && (
        <span
          className={`pointer-events-none absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-rose-500 px-1 text-[8px] font-black tabular-nums leading-none text-white shadow-sm ring-2 ring-zinc-50`}
          aria-hidden
        >
          {badge > 99 ? '99+' : badge}
        </span>
      )}
    </Button>
  )

  return (
    <div
      className={`${RAIL_W} flex h-full shrink-0 flex-col border-l border-zinc-100 bg-zinc-50/40 px-1.5 py-2 gap-1`}
    >
      <Button
        type="button"
        variant="ghost"
        title={expanded ? 'Collapse project panel' : 'Expand project panel'}
        aria-label={expanded ? 'Collapse project panel' : 'Expand project panel'}
        aria-pressed={expanded}
        onClick={() => onExpandedChange(!expanded)}
        className="h-9 w-full shrink-0 rounded-md border border-zinc-300/80 bg-zinc-200/80 px-0 font-semibold text-zinc-700 shadow-sm hover:bg-zinc-200 hover:text-darkDelegation"
      >
        {expanded ? (
          <PanelRightClose className="size-[18px]" strokeWidth={2.25} aria-hidden />
        ) : (
          <PanelLeftOpen className="size-[18px]" strokeWidth={2.25} aria-hidden />
        )}
      </Button>

      <div className="flex flex-col items-center gap-1">
        {tabBtn('overview', 'Project overview', <Info size={18} strokeWidth={2.25} />)}
        {tabBtn('activity', 'Activity', <Activity size={18} strokeWidth={2.25} />, activityAttentionCount)}
        {tabBtn(
          'agents',
          agentsAttentionCount > 0
            ? `Agents, ${agentsAttentionCount} ${agentsAttentionCount === 1 ? 'chat needs' : 'chats need'} your attention`
            : 'Agents',
          <Users size={18} strokeWidth={2.25} />,
          agentsAttentionCount,
        )}
      </div>
    </div>
  )
}
