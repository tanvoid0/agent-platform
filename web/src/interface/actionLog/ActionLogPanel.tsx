import React, { useEffect, useMemo, useRef, useState } from 'react'
import { getAllAgents } from '../../data/agents'
import { useCoreStore } from '../../integration/store/coreStore'
import { useActiveTeam } from '../../integration/store/teamStore'
import { DeliverablesPanel } from '../DeliverablesPanel'
import { ActionLogIconRail } from './ActionLogIconRail'
import { ActionLogPanelHeader } from './ActionLogPanelHeader'
import type { ActionLogMainTab } from './actionLogMainTab'
import { ActivityLogEntries } from './ActivityLogEntries'
import { buildTechnicalLogExportText } from './buildTechnicalLogExport'
import { buildRequestPairingMap } from './buildRequestResponsePairing'
import { DebugLogEntryView } from './DebugLogEntryView'

const ACTION_LOG_MAIN_TAB_KEY = 'ui:action-log-main-tab'
const RAIL_PX = 52

export type ActionLogPanelProps = {
  leftLogExpanded: boolean
  onLeftLogExpandedChange: (next: boolean) => void
  /** Total width in px when expanded, including the 52px icon rail. */
  expandedTotalWidthPx: number
}

function readStoredActionLogMainTab(): ActionLogMainTab {
  try {
    const raw = localStorage.getItem(ACTION_LOG_MAIN_TAB_KEY)
    if (raw === 'activity' || raw === 'technical' || raw === 'deliverables') {
      return raw
    }
    if (raw === 'agents') return 'activity'
  } catch {
    /* ignore */
  }
  return 'technical'
}

export function ActionLogPanel({
  leftLogExpanded,
  onLeftLogExpandedChange,
  expandedTotalWidthPx,
}: ActionLogPanelProps) {
  const { setLogOpen, actionLog, debugLog, logFilterAgentIndex } = useCoreStore()
  const activeTeam = useActiveTeam()
  const agents = getAllAgents(activeTeam)
  const [activeTab, setActiveTab] = useState<ActionLogMainTab>(readStoredActionLogMainTab)
  const [isFilterMenuOpen, setIsFilterMenuOpen] = useState(false)
  const topRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    try {
      localStorage.setItem(ACTION_LOG_MAIN_TAB_KEY, activeTab)
    } catch {
      /* ignore */
    }
  }, [activeTab])

  const handleDownloadAll = () => {
    const content = buildTechnicalLogExportText(debugLog, agents)
    const blob = new Blob([content], { type: 'text/plain' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `agent-platform-technical-logs-${Date.now()}.txt`
    a.click()
    URL.revokeObjectURL(url)
  }

  useEffect(() => {
    setIsFilterMenuOpen(false)
  }, [activeTab])

  useEffect(() => {
    if (activeTab !== 'activity' && activeTab !== 'technical') return
    setTimeout(() => topRef.current?.scrollIntoView({ behavior: 'smooth' }), 100)
  }, [actionLog, debugLog, activeTab])

  const filterAgent =
    logFilterAgentIndex !== null
      ? (agents.find((a) => a.index === logFilterAgentIndex) ?? null)
      : null

  const entries =
    logFilterAgentIndex !== null
      ? actionLog.filter((e) => e.agentIndex === logFilterAgentIndex).reverse()
      : [...actionLog].reverse()

  const debugEntries =
    logFilterAgentIndex !== null
      ? debugLog.filter((e) => e.agentIndex === logFilterAgentIndex).reverse()
      : [...debugLog].reverse()

  const requestPairingMap = useMemo(() => {
    const chronological =
      logFilterAgentIndex !== null
        ? debugLog.filter((e) => e.agentIndex === logFilterAgentIndex)
        : debugLog
    return buildRequestPairingMap(chronological)
  }, [debugLog, logFilterAgentIndex])

  const pickFilter = (index: number | null) => {
    setLogOpen(true, index)
    setIsFilterMenuOpen(false)
  }

  const totalWidth = leftLogExpanded ? expandedTotalWidthPx : RAIL_PX
  const contentWidth = Math.max(0, expandedTotalWidthPx - RAIL_PX)

  return (
    <div
      className="h-full bg-white border-r border-zinc-100 flex flex-row pointer-events-auto overflow-hidden shrink-0 relative"
      style={{ width: totalWidth }}
    >
      <ActionLogIconRail
        activeTab={activeTab}
        onTabChange={setActiveTab}
        expanded={leftLogExpanded}
        onExpandedChange={onLeftLogExpandedChange}
      />

      {leftLogExpanded && (
        <div
          className="flex flex-col flex-1 min-h-0 min-w-0 overflow-hidden"
          style={{ width: contentWidth }}
        >
          <ActionLogPanelHeader
            filterAgent={filterAgent}
            onClearFilter={() => setLogOpen(true, null)}
            activeTab={activeTab}
            logFilterAgentIndex={logFilterAgentIndex}
            isFilterMenuOpen={isFilterMenuOpen}
            onToggleFilterMenu={() => setIsFilterMenuOpen(!isFilterMenuOpen)}
            onCloseFilterMenu={() => setIsFilterMenuOpen(false)}
            onSelectFilterAgent={pickFilter}
            agents={agents}
            showDownloadTechnical={activeTab === 'technical' && debugEntries.length > 0}
            onDownloadTechnical={handleDownloadAll}
          />

          <div className="flex-1 overflow-y-auto p-5 space-y-4 shadow-[inset_0_-20px_20px_-20px_rgba(0,0,0,0.05)] min-h-0">
            <div ref={topRef} />

            {activeTab === 'activity' ? (
              <ActivityLogEntries entries={entries} agents={agents} />
            ) : activeTab === 'technical' ? (
              debugEntries.length === 0 ? (
                <p className="text-zinc-300 text-[10px] font-bold uppercase tracking-widest text-center py-16">
                  No technical data...
                </p>
              ) : (
                debugEntries.map((entry) => (
                  <DebugLogEntryView
                    key={entry.id}
                    entry={entry}
                    requestPairing={
                      entry.phase === 'request'
                        ? requestPairingMap.get(entry.id)
                        : undefined
                    }
                  />
                ))
              )
            ) : (
              <DeliverablesPanel />
            )}
          </div>
        </div>
      )}
    </div>
  )
}
