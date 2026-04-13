import { Activity, Info } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'

export type ProjectSideTab = 'overview' | 'activity' | 'agents'

export const ProjectSideTabs: React.FC<{
  sideTab: ProjectSideTab
  onSideTab: (tab: ProjectSideTab) => void
  activityAttentionCount: number
}> = ({ sideTab, onSideTab, activityAttentionCount }) => (
  <div className="grid grid-cols-2 p-1 mb-6 rounded-xl bg-zinc-100 border border-zinc-100/80 gap-1">
    <Button
      type="button"
      variant="ghost"
      onClick={() => onSideTab('overview')}
      className={`flex h-auto items-center justify-center gap-1.5 rounded-lg py-2 text-[9px] font-black uppercase tracking-widest ${
        sideTab === 'overview'
          ? 'bg-white text-darkDelegation shadow-sm hover:bg-white'
          : 'text-zinc-400 hover:text-zinc-600'
      }`}
    >
      <Info size={12} strokeWidth={2.5} />
      Overview
    </Button>
    <Button
      type="button"
      variant="ghost"
      onClick={() => onSideTab('activity')}
      className={`relative flex h-auto items-center justify-center gap-1.5 overflow-visible rounded-lg py-2 text-[9px] font-black uppercase tracking-widest ${
        sideTab === 'activity'
          ? 'bg-white text-darkDelegation shadow-sm hover:bg-white'
          : 'text-zinc-400 hover:text-zinc-600'
      }`}
      aria-label={
        activityAttentionCount > 0
          ? `Activity, ${activityAttentionCount} ${activityAttentionCount === 1 ? 'item needs' : 'items need'} your attention`
          : 'Activity'
      }
    >
      <Activity size={12} strokeWidth={2.5} />
      Activity
      {activityAttentionCount > 0 && (
        <span
          className={`pointer-events-none absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-rose-500 px-1 text-[8px] font-black tabular-nums leading-none text-white shadow-sm ring-2 ${
            sideTab === 'activity' ? 'ring-white' : 'ring-zinc-100'
          }`}
          aria-hidden
        >
          {activityAttentionCount > 99 ? '99+' : activityAttentionCount}
        </span>
      )}
    </Button>
  </div>
)
