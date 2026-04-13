import { CheckCircle2, ExternalLink } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import { ModalBackdrop, ModalRoot } from '../components/ModalChrome'
import type { TeamProjectReviewDraft } from './chatReviewTypes'

export const ChatReviewFlowModal: React.FC<{
  reviewDraft: TeamProjectReviewDraft
  teamReviewDone: boolean
  projectReviewDone: boolean
  isApplyingReviewedSetup: boolean
  onClose: () => void
  onOpenTeamDraft: (teamId: string) => void
  onOpenProjectDraft: (projectId: string) => void
  onToggleTeamReview: () => void
  onToggleProjectReview: () => void
  onApply: () => void
}> = ({
  reviewDraft,
  teamReviewDone,
  projectReviewDone,
  isApplyingReviewedSetup,
  onClose,
  onOpenTeamDraft,
  onOpenProjectDraft,
  onToggleTeamReview,
  onToggleProjectReview,
  onApply,
}) => (
  <ModalRoot layer="modalAlert" paddingClassName="p-4">
    <ModalBackdrop tone="dim" onRequestClose={onClose} />
    <div
      className="relative w-full max-w-xl rounded-3xl border border-zinc-200 bg-white shadow-2xl p-6"
      onMouseDown={(e) => e.stopPropagation()}
    >
      <div className="flex items-start justify-between gap-4 mb-5">
        <div>
          <h3 className="text-sm font-black uppercase tracking-widest text-darkDelegation">Guided review</h3>
          <p className="text-xs text-zinc-500 mt-1">
            Review both drafts, then apply them together to the simulation.
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          onClick={onClose}
          disabled={isApplyingReviewedSetup}
          className="rounded-lg px-3 py-1.5 text-[10px] font-black uppercase tracking-wider text-zinc-500 hover:bg-zinc-100 disabled:opacity-50"
        >
          Close
        </Button>
      </div>

      <div className="space-y-3">
        <div className="rounded-2xl border border-zinc-200 bg-zinc-50/70 p-4">
          <div className="flex items-center justify-between gap-3">
            <p className="text-[11px] font-black uppercase tracking-wider text-zinc-700">
              1) Review team draft
            </p>
            {teamReviewDone && <CheckCircle2 size={16} className="text-emerald-600" />}
          </div>
          <p className="text-[12px] text-zinc-600 mt-1">{reviewDraft.teamName}</p>
          <div className="flex items-center gap-2 mt-3">
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenTeamDraft(reviewDraft.teamId)}
              className="flex items-center gap-1.5 rounded-xl border-zinc-200 bg-white px-3 py-2 text-[10px] font-black uppercase tracking-wider hover:bg-zinc-50"
            >
              <ExternalLink size={12} />
              Open team draft
            </Button>
            <Button
              type="button"
              variant="secondary"
              onClick={onToggleTeamReview}
              className={`rounded-xl px-3 py-2 text-[10px] font-black uppercase tracking-wider ${
                teamReviewDone
                  ? 'bg-emerald-100 text-emerald-700 hover:bg-emerald-100'
                  : 'bg-zinc-200 text-zinc-700 hover:bg-zinc-200'
              }`}
            >
              {teamReviewDone ? 'Marked reviewed' : 'Mark as reviewed'}
            </Button>
          </div>
        </div>

        <div className="rounded-2xl border border-zinc-200 bg-zinc-50/70 p-4">
          <div className="flex items-center justify-between gap-3">
            <p className="text-[11px] font-black uppercase tracking-wider text-zinc-700">
              2) Review project draft
            </p>
            {projectReviewDone && <CheckCircle2 size={16} className="text-emerald-600" />}
          </div>
          <p className="text-[12px] text-zinc-600 mt-1">{reviewDraft.projectTitle}</p>
          <div className="flex items-center gap-2 mt-3">
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenProjectDraft(reviewDraft.projectId)}
              className="flex items-center gap-1.5 rounded-xl border-zinc-200 bg-white px-3 py-2 text-[10px] font-black uppercase tracking-wider hover:bg-zinc-50"
            >
              <ExternalLink size={12} />
              Open project draft
            </Button>
            <Button
              type="button"
              variant="secondary"
              onClick={onToggleProjectReview}
              className={`rounded-xl px-3 py-2 text-[10px] font-black uppercase tracking-wider ${
                projectReviewDone
                  ? 'bg-emerald-100 text-emerald-700 hover:bg-emerald-100'
                  : 'bg-zinc-200 text-zinc-700 hover:bg-zinc-200'
              }`}
            >
              {projectReviewDone ? 'Marked reviewed' : 'Mark as reviewed'}
            </Button>
          </div>
        </div>
      </div>

      <Button
        type="button"
        onClick={() => void onApply()}
        disabled={!teamReviewDone || !projectReviewDone || isApplyingReviewedSetup}
        className="mt-5 w-full rounded-2xl bg-darkDelegation px-4 py-3 text-[10px] font-black uppercase tracking-widest text-white hover:bg-black disabled:pointer-events-none disabled:opacity-40"
      >
        {isApplyingReviewedSetup ? 'Applying...' : '3) Apply reviewed team + project'}
      </Button>
    </div>
  </ModalRoot>
)
