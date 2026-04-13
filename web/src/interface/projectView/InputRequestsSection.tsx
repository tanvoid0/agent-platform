import { ClipboardList, MessageSquare, MessageSquareWarning, Siren } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import { USER_COLOR, USER_COLOR_LIGHT, USER_COLOR_SOFT } from '../../theme/brand'
import type { InputRequestItem } from './buildInputRequestList'

export const InputRequestsSection: React.FC<{
  items: InputRequestItem[]
  characters: { index: number; name: string; color: string }[]
  selectedNpcIndex: number | null
  activeAuditTaskId: string | null
  onItemClick: (item: InputRequestItem) => void
  onApproveAll?: () => void
}> = ({ items, characters, selectedNpcIndex, activeAuditTaskId, onItemClick, onApproveAll }) => {
  const approvableCount = items.filter((i) => i.kind === 'proposed_task' || i.kind === 'review').length
  return (
  <div className="mb-6">
    <div className="flex items-center gap-2 mb-4 flex-wrap">
      <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Needs your input</p>
      <div className="h-px flex-1 bg-zinc-100 min-w-[2rem]" />
      {onApproveAll && approvableCount > 0 && (
        <Button
          type="button"
          variant="outline"
          onClick={onApproveAll}
          className="rounded-lg px-2.5 py-1 h-auto text-[9px] font-black uppercase tracking-widest border-emerald-200/90 text-emerald-800 bg-emerald-50/80 hover:bg-emerald-100/90 shrink-0"
          title="Approve every proposed plan and every submission in review listed below"
        >
          Approve all ({approvableCount})
        </Button>
      )}
      <span className="text-[9px] font-mono font-bold text-zinc-400 tabular-nums">{items.length}</span>
    </div>
    <ul className="flex flex-col gap-1.5">
      {items.map((item) => {
        const agentColor = characters.find((c) => c.index === item.agentIndex)?.color ?? '#71717a'
        const isActive =
          selectedNpcIndex === item.agentIndex &&
          (item.kind === 'brief' ||
            item.kind === 'chat_reply' ||
            ((item.kind === 'review' || item.kind === 'proposed_task') && activeAuditTaskId === item.taskId))
        return (
          <li key={item.id}>
            <Button
              type="button"
              variant="ghost"
              onClick={() => onItemClick(item)}
              className={`flex h-auto w-full items-start justify-start gap-2.5 rounded-lg border p-2.5 text-left font-normal active:scale-[0.99] ${
                isActive
                  ? 'border-darkDelegation/25 bg-zinc-50 shadow-sm ring-1 ring-darkDelegation/10 hover:bg-zinc-50'
                  : 'border-zinc-100 bg-white/80 hover:border-zinc-200 hover:bg-white'
              }`}
            >
              <div
                className="shrink-0 w-8 h-8 rounded-lg flex items-center justify-center border"
                style={
                  item.kind === 'brief'
                    ? {
                        backgroundColor: '#fafafa',
                        borderColor: '#e4e4e7',
                        color: '#18181b',
                      }
                    : item.kind === 'proposed_task'
                      ? {
                          backgroundColor: '#eef2ff',
                          borderColor: '#c7d2fe',
                          color: '#4f46e5',
                        }
                      : item.kind === 'chat_reply'
                        ? {
                            backgroundColor: '#fff7ed',
                            borderColor: '#fed7aa',
                            color: '#c2410c',
                          }
                        : {
                            backgroundColor: USER_COLOR_LIGHT,
                            borderColor: USER_COLOR_SOFT,
                            color: USER_COLOR,
                          }
                }
              >
                {item.kind === 'brief' ? (
                  <Siren size={18} strokeWidth={2.5} />
                ) : item.kind === 'proposed_task' ? (
                  <ClipboardList size={18} strokeWidth={2.5} />
                ) : item.kind === 'chat_reply' ? (
                  <MessageSquare size={18} strokeWidth={2.5} />
                ) : (
                  <MessageSquareWarning size={18} strokeWidth={2.5} />
                )}
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 mb-0.5">
                  <span
                    className="w-1.5 h-1.5 rounded-full shrink-0"
                    style={{ backgroundColor: agentColor }}
                  />
                  <span className="text-[10px] font-black text-darkDelegation uppercase tracking-tight truncate">
                    {item.title}
                  </span>
                  <span className="text-[8px] font-black uppercase tracking-widest text-zinc-400 shrink-0">
                    {item.kind === 'brief'
                      ? 'Brief'
                      : item.kind === 'proposed_task'
                        ? 'Approve plan'
                        : item.kind === 'chat_reply'
                          ? 'Chat'
                          : 'Review'}
                  </span>
                </div>
                <p className="text-[10px] font-medium text-zinc-600 leading-snug line-clamp-2">
                  {item.detail}
                </p>
              </div>
            </Button>
          </li>
        )
      })}
    </ul>
  </div>
  )
}
