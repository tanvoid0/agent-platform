import { Copy, Trash2 } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import type { AgenticSystem, AgentNode } from '../../data/agents'
import { AgentPresenceBadge } from '../components/AgentPresenceBadge'
import { Avatar } from '../components/Avatar'

export const ChatPanelHeader: React.FC<{
  agent: AgentNode
  selectedNpcIndex: number
  activeTeam: AgenticSystem
  isThinking: boolean
  hasVisibleMessages: boolean
  onCopyConversation: () => void
  onClearChat: () => void
}> = ({
  agent,
  selectedNpcIndex,
  activeTeam,
  isThinking,
  hasVisibleMessages,
  onCopyConversation,
  onClearChat,
}) => (
  <div className="flex shrink-0 items-center justify-between gap-2 border-b border-zinc-100 px-2 py-1.5">
    <div className="flex min-w-0 items-center gap-2 pl-0.5">
      <div className="shrink-0 rounded-xl border border-zinc-100 bg-zinc-50/80 p-0.5">
        <Avatar
          type={agent.index === activeTeam.leadAgent.index ? 'lead' : 'sub'}
          color={agent.color}
          size={28}
        />
      </div>
      <div className="flex min-w-0 flex-col gap-0.5 leading-tight">
        <span className="truncate text-[11px] font-black text-darkDelegation">{agent.name}</span>
        <AgentPresenceBadge agentIndex={selectedNpcIndex} size="sm" />
      </div>
    </div>
    <div className="flex shrink-0 items-center gap-0.5">
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={!hasVisibleMessages}
        title="Copy full conversation as plain text"
        onClick={onCopyConversation}
        className="h-8 shrink-0 gap-1.5 rounded-lg px-2 text-[10px] font-black uppercase tracking-wider text-zinc-500 hover:bg-zinc-100 hover:text-zinc-800 disabled:opacity-40"
      >
        <Copy size={14} className="shrink-0 opacity-70" />
        Copy all
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={isThinking || !hasVisibleMessages}
        title={isThinking ? 'Wait for the reply to finish' : 'Remove all messages in this thread'}
        onClick={onClearChat}
        className="h-8 shrink-0 gap-1.5 rounded-lg px-2 text-[10px] font-black uppercase tracking-wider text-zinc-500 hover:bg-zinc-100 hover:text-zinc-800 disabled:opacity-40"
      >
        <Trash2 size={14} className="shrink-0 opacity-70" />
        Clear chat
      </Button>
    </div>
  </div>
)
