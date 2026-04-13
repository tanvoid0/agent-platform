import { Download, Filter } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import { AgentPresenceBadge } from '../components/AgentPresenceBadge'
import type { ActionLogMainTab } from './actionLogMainTab'

export type ActionLogHeaderAgent = { index: number; name: string; color: string }

export const ActionLogPanelHeader: React.FC<{
  filterAgent: ActionLogHeaderAgent | null
  onClearFilter: () => void
  activeTab: ActionLogMainTab
  logFilterAgentIndex: number | null
  isFilterMenuOpen: boolean
  onToggleFilterMenu: () => void
  onCloseFilterMenu: () => void
  onSelectFilterAgent: (index: number | null) => void
  agents: ActionLogHeaderAgent[]
  showDownloadTechnical: boolean
  onDownloadTechnical: () => void
}> = ({
  filterAgent,
  onClearFilter,
  activeTab,
  logFilterAgentIndex,
  isFilterMenuOpen,
  onToggleFilterMenu,
  onCloseFilterMenu,
  onSelectFilterAgent,
  agents,
  showDownloadTechnical,
  onDownloadTechnical,
}) => (
  <div className="h-10 px-5 border-b border-zinc-100 flex items-center justify-between bg-white shrink-0 z-10">
    <div className="flex items-center gap-2">
      <span className="text-[11px] font-bold text-zinc-500 uppercase tracking-wider">Logs</span>
      {filterAgent && (
        <div
          className="flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[9px] font-bold text-white uppercase tracking-tighter animate-in fade-in zoom-in duration-200"
          style={{ backgroundColor: filterAgent.color }}
        >
          {filterAgent.name}
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            onClick={onClearFilter}
            className="size-4 min-w-0 p-0 font-bold hover:scale-110"
          >
            ×
          </Button>
        </div>
      )}
    </div>

    <div className="flex items-center gap-2">
      <div className="relative">
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onToggleFilterMenu}
          disabled={activeTab !== 'activity' && activeTab !== 'technical'}
          className={`rounded disabled:pointer-events-none disabled:opacity-30 ${
            isFilterMenuOpen || logFilterAgentIndex !== null
              ? 'bg-darkDelegation text-white hover:bg-darkDelegation hover:text-white'
              : 'text-zinc-400 hover:bg-zinc-50 hover:text-darkDelegation'
          }`}
          title="Filter by agent"
        >
          <Filter size={14} />
        </Button>

        {isFilterMenuOpen && (
          <>
            <div className="fixed inset-0 z-20" onClick={onCloseFilterMenu} />
            <div className="absolute right-0 mt-2 w-48 bg-white border border-zinc-100 rounded-xl shadow-xl z-30 py-1.5 overflow-hidden animate-in fade-in slide-in-from-top-2 duration-200">
              <Button
                type="button"
                variant="ghost"
                onClick={() => onSelectFilterAgent(null)}
                className={`h-auto w-full justify-start rounded-none px-4 py-2 text-left text-[10px] font-bold uppercase tracking-widest ${
                  logFilterAgentIndex === null ? 'text-darkDelegation' : 'text-zinc-400'
                }`}
              >
                <div
                  className={`w-2 h-2 rounded-full ${
                    logFilterAgentIndex === null ? 'bg-darkDelegation' : 'bg-transparent border border-zinc-200'
                  }`}
                />
                All Agents
              </Button>
              <div className="h-px bg-zinc-50 my-1" />
              {agents.map((agent) => (
                <Button
                  key={agent.index}
                  type="button"
                  variant="ghost"
                  onClick={() => onSelectFilterAgent(agent.index)}
                  className={`flex h-auto w-full flex-col items-stretch gap-1 rounded-none px-4 py-2 text-left text-[10px] font-bold uppercase tracking-widest ${
                    logFilterAgentIndex === agent.index ? 'text-darkDelegation' : 'text-zinc-400'
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <div
                      className="h-2 w-2 shrink-0 rounded-full"
                      style={{ backgroundColor: agent.color }}
                    />
                    {agent.name}
                  </span>
                  <AgentPresenceBadge agentIndex={agent.index} size="sm" className="pl-4" />
                </Button>
              ))}
            </div>
          </>
        )}
      </div>

      {showDownloadTechnical && (
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onDownloadTechnical}
          className="text-zinc-400 hover:bg-zinc-50 hover:text-darkDelegation"
          title="Download all as .txt"
        >
          <Download size={14} />
        </Button>
      )}
    </div>
  </div>
)
