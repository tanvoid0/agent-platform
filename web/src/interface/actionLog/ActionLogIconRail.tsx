import {
  Activity,
  FolderOpen,
  PanelLeftClose,
  PanelRightOpen,
  Terminal,
} from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import type { ActionLogMainTab } from './actionLogMainTab'

const RAIL_W = 'w-[52px]'

export const ActionLogIconRail: React.FC<{
  activeTab: ActionLogMainTab
  onTabChange: (tab: ActionLogMainTab) => void
  expanded: boolean
  onExpandedChange: (next: boolean) => void
}> = ({ activeTab, onTabChange, expanded, onExpandedChange }) => {
  const selectTab = (tab: ActionLogMainTab) => {
    if (!expanded) {
      onExpandedChange(true)
    }
    onTabChange(tab)
  }

  const tabBtn = (
    tab: ActionLogMainTab,
    label: string,
    icon: React.ReactNode,
    extra?: React.ReactNode,
  ) => (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      title={label}
      aria-label={label}
      aria-pressed={activeTab === tab}
      onClick={() => selectTab(tab)}
      className={`relative size-10 shrink-0 rounded-lg ${
        activeTab === tab
          ? 'bg-zinc-100 text-darkDelegation'
          : 'text-zinc-400 hover:bg-zinc-50 hover:text-zinc-600'
      }`}
    >
      {icon}
      {extra}
    </Button>
  )

  return (
    <div
      className={`${RAIL_W} flex h-full shrink-0 flex-col border-r border-zinc-100 bg-zinc-50/40 px-1.5 py-2 gap-1`}
    >
      <Button
        type="button"
        variant="ghost"
        title={expanded ? 'Collapse log panel' : 'Expand log panel'}
        aria-label={expanded ? 'Collapse log panel' : 'Expand log panel'}
        aria-pressed={expanded}
        onClick={() => onExpandedChange(!expanded)}
        className="h-9 w-full shrink-0 rounded-md border border-zinc-300/80 bg-zinc-200/80 px-0 font-semibold text-zinc-700 shadow-sm hover:bg-zinc-200 hover:text-darkDelegation"
      >
        {expanded ? (
          <PanelLeftClose className="size-[18px]" strokeWidth={2.25} aria-hidden />
        ) : (
          <PanelRightOpen className="size-[18px]" strokeWidth={2.25} aria-hidden />
        )}
      </Button>

      <div className="flex flex-col items-center gap-1">
        {tabBtn('activity', 'Activity', <Activity size={18} strokeWidth={2.25} />)}
        {tabBtn('technical', 'Technical', <Terminal size={18} strokeWidth={2.25} />)}
        {tabBtn('deliverables', 'Deliverables', <FolderOpen size={18} strokeWidth={2.25} />)}
      </div>
    </div>
  )
}
