import { RefreshCcw, RotateCcw } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import type { ProjectPhase } from '../../integration/store/coreStore'

export const ProjectResetActions: React.FC<{
  phase: ProjectPhase
  serverProjectsEnabled: boolean
  onImprove: () => void
  onOpenReset: () => void
}> = ({ phase, serverProjectsEnabled, onImprove, onOpenReset }) => (
  <div className="mb-8 w-full">
    {phase === 'done' && (
      <Button
        type="button"
        variant="outline"
        onClick={onImprove}
        className="group mb-2 flex w-full items-center justify-center gap-2 rounded-2xl border-emerald-200 bg-emerald-50 px-4 py-3 text-emerald-700 hover:border-emerald-300 hover:bg-emerald-100 active:scale-[0.98]"
        title="Reopen this project and continue from current output"
      >
        <RotateCcw size={13} strokeWidth={3} className="transition-transform group-hover:-rotate-45 duration-300" />
        <span className="text-[10px] font-black uppercase tracking-widest">Improve / Extend</span>
      </Button>
    )}
    <Button
      type="button"
      onClick={onOpenReset}
      className={`group flex w-full items-center justify-center gap-2 rounded-2xl px-4 py-4 active:scale-[0.98] ${
        phase === 'done'
          ? 'bg-darkDelegation text-white shadow-xl shadow-darkDelegation/10 hover:bg-black'
          : 'bg-zinc-100 text-zinc-400 hover:bg-zinc-200 hover:text-zinc-600'
      }`}
    >
      <RefreshCcw size={14} strokeWidth={3} className="transition-transform group-hover:rotate-180 duration-500" />
      <span className="text-[10px] font-black uppercase tracking-widest">
        {serverProjectsEnabled ? 'New or reset…' : 'Start New Project'}
      </span>
    </Button>
  </div>
)
