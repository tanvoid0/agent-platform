import { MessageSquare } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'

export type UnreadChatRow = {
  agentIndex: number
  name: string
  color: string
  count: number
}

export const UnreadMessagesSection: React.FC<{
  rows: UnreadChatRow[]
  totalUnread: number
  selectedNpcIndex: number | null
  isChatting: boolean
  onOpenAgentChat: (agentIndex: number) => void
}> = ({ rows, totalUnread, selectedNpcIndex, isChatting, onOpenAgentChat }) => (
  <div className="mb-6">
    <div className="flex items-center gap-2 mb-4">
      <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Unread messages</p>
      <div className="h-px flex-1 bg-zinc-100" />
      <span className="text-[9px] font-mono font-bold text-zinc-400 tabular-nums">{totalUnread}</span>
    </div>
    <ul className="flex flex-col gap-1.5">
      {rows.map((row) => {
        const isActive = selectedNpcIndex === row.agentIndex && isChatting
        return (
          <li key={`unread-${row.agentIndex}`}>
            <Button
              type="button"
              variant="ghost"
              onClick={() => onOpenAgentChat(row.agentIndex)}
              className={`flex h-auto w-full items-start justify-start gap-2.5 rounded-lg border p-2.5 text-left font-normal active:scale-[0.99] ${
                isActive
                  ? 'border-darkDelegation/25 bg-zinc-50 shadow-sm ring-1 ring-darkDelegation/10 hover:bg-zinc-50'
                  : 'border-zinc-100 bg-white/80 hover:border-zinc-200 hover:bg-white'
              }`}
            >
              <div className="shrink-0 w-8 h-8 rounded-lg flex items-center justify-center border border-sky-100 bg-sky-50 text-sky-600">
                <MessageSquare size={18} strokeWidth={2.5} />
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 mb-0.5">
                  <span
                    className="w-1.5 h-1.5 rounded-full shrink-0"
                    style={{ backgroundColor: row.color }}
                  />
                  <span className="text-[10px] font-black text-darkDelegation uppercase tracking-tight truncate">
                    {row.name}
                  </span>
                  <span className="text-[8px] font-black uppercase tracking-widest text-zinc-400 shrink-0">
                    Chat
                  </span>
                </div>
                <p className="text-[10px] font-medium text-zinc-600 leading-snug">
                  {row.count === 1
                    ? '1 new reply — open chat to read'
                    : `${row.count} new replies — open chat to read`}
                </p>
              </div>
            </Button>
          </li>
        )
      })}
    </ul>
  </div>
)
