import { Info } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import { getAllAgents, type AgenticSystem } from '../../data/agents'
import { AgentPresenceBadge } from '../components/AgentPresenceBadge'
import { formatTokens } from '../formatTokens'

export const TokenUsageSection: React.FC<{
  activeTeam: AgenticSystem
  totalEstimatedCost: number
  totalTokenUsage: { totalTokens: number; promptTokens: number; completionTokens: number }
  agentTokenUsage: Record<string, { totalTokens: number; promptTokens: number; completionTokens: number }>
  agentEstimatedCost: Record<number, number>
  onOpenPricing: () => void
}> = ({
  activeTeam,
  totalEstimatedCost,
  totalTokenUsage,
  agentTokenUsage,
  agentEstimatedCost,
  onOpenPricing,
}) => (
  <div className="mb-8">
    <div className="flex items-center justify-between mb-4">
      <div className="flex items-center gap-2 flex-1">
        <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Token Usage</p>
        <div className="h-px flex-1 bg-zinc-100" />
      </div>
      <Button
        type="button"
        variant="outline"
        onClick={onOpenPricing}
        className="group ml-4 flex items-center gap-2 rounded-lg border-emerald-100 bg-emerald-50 px-2.5 py-1 hover:border-emerald-200 hover:bg-emerald-100 active:scale-95"
      >
        <span className="text-[10px] font-black uppercase tracking-tight text-emerald-600">
          Total Est. ${totalEstimatedCost.toFixed(3)}
        </span>
        <Info size={11} className="text-emerald-500 group-hover:text-emerald-600" />
      </Button>
    </div>

    <div className="bg-zinc-50 rounded-xl p-5 border border-zinc-100 mb-6">
      <div className="flex flex-col gap-1 mb-6">
        <span className="text-4xl font-mono font-black text-darkDelegation tracking-tighter">
          {formatTokens(totalTokenUsage.totalTokens)}
        </span>
      </div>
      <div className="flex items-center gap-1.5 text-[11px] font-bold font-mono">
        <span className="text-zinc-700">
          {formatTokens(totalTokenUsage.promptTokens)}{' '}
          <span className="text-zinc-400 font-medium">input</span>
        </span>
        <span className="text-zinc-300">+</span>
        <span className="text-zinc-700">
          {formatTokens(totalTokenUsage.completionTokens)}{' '}
          <span className="text-zinc-400 font-medium">output</span>
        </span>
      </div>
    </div>

    <div className="space-y-1">
      {(Object.entries(agentTokenUsage) as Array<
        [string, { totalTokens: number; promptTokens: number; completionTokens: number }]
      >)
        .sort(([, a], [, b]) => b.totalTokens - a.totalTokens)
        .map(([idx, usage]) => {
          const agentIndex = parseInt(idx, 10)
          const agents = getAllAgents(activeTeam)
          const agent =
            agentIndex === -1
              ? { name: 'System', color: '#71717a' }
              : agents.find((a) => a.index === agentIndex)

          if (!agent || usage.totalTokens === 0) return null

          return (
            <div
              key={idx}
              className="flex items-center justify-between py-2 px-2 hover:bg-zinc-100/50 rounded-lg transition-colors group"
            >
              <div className="flex items-center gap-3">
                <div
                  className="w-1.5 h-1.5 rounded-full shadow-[0_0_8px_rgba(0,0,0,0.1)]"
                  style={{ backgroundColor: agent.color }}
                />
                <div className="flex min-w-0 flex-col gap-0.5">
                  <span className="text-[11px] font-bold uppercase tracking-tight text-zinc-600 transition-colors group-hover:text-darkDelegation">
                    {agent.name}
                  </span>
                  {agentIndex >= 0 ? <AgentPresenceBadge agentIndex={agentIndex} size="sm" /> : null}
                </div>
              </div>
              <div className="flex flex-col items-end gap-1">
                <div className="flex items-center gap-2">
                  {agentEstimatedCost[agentIndex] > 0 && (
                    <span className="text-[9px] font-mono font-bold text-emerald-600/70">
                      ${agentEstimatedCost[agentIndex].toFixed(4)}
                    </span>
                  )}
                  <span className="text-[11px] font-mono font-black text-darkDelegation">
                    {formatTokens(usage.totalTokens)}
                  </span>
                </div>
                <div className="flex items-center gap-1 text-[9px] font-bold font-mono text-zinc-400">
                  <span>
                    {formatTokens(usage.promptTokens)}{' '}
                    <span className="font-medium opacity-60">input</span>
                  </span>
                  <span className="text-zinc-200">+</span>
                  <span>
                    {formatTokens(usage.completionTokens)}{' '}
                    <span className="font-medium opacity-60">output</span>
                  </span>
                </div>
              </div>
            </div>
          )
        })}
    </div>
  </div>
)
