import React from 'react'
import type { ActionLogEntry } from '../../integration/store/coreStore'
import { buildActivityLogCopyText } from './buildActivityLogCopyText'
import { classifyActivityLogAction } from './classifyActivityLog'
import { CopyButton } from './CopyButton'
import { formatLogTimeShort } from './formatLogTime'

export type ActivityLogAgentRef = { index: number; name: string; color?: string }

export const ActivityLogEntries: React.FC<{
  entries: ActionLogEntry[]
  agents: ActivityLogAgentRef[]
}> = ({ entries, agents }) => {
  if (entries.length === 0) {
    return (
      <p className="text-zinc-300 text-[10px] font-bold uppercase tracking-widest text-center py-16">
        Awaiting actions...
      </p>
    )
  }

  return (
    <ul className="flex flex-col gap-3 list-none p-0 m-0">
      {entries.map((entry) => {
        const agent = agents.find((a) => a.index === entry.agentIndex)
        const agentLabel = agent?.name ?? 'System'
        const presentation = classifyActivityLogAction(entry.action)
        const Icon = presentation.Icon
        const copyText = buildActivityLogCopyText(entry, agentLabel)

        return (
          <li key={entry.id}>
            <article
              className={`rounded-xl border p-3 shadow-sm transition-shadow hover:shadow-md ${presentation.cardClass}`}
            >
              <div className="flex items-start gap-2.5 min-w-0">
                <div
                  className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-lg ${presentation.iconWrapClass}`}
                  aria-hidden
                >
                  <Icon size={18} strokeWidth={2.25} />
                </div>

                <div className="min-w-0 flex-1 flex flex-col gap-2">
                  <div className="flex items-start justify-between gap-2 min-w-0">
                    <div className="flex flex-wrap items-center gap-1.5 gap-y-1 min-w-0">
                      <span
                        className={`inline-flex items-center rounded-md border px-1.5 py-0.5 text-[8px] font-black uppercase tracking-widest ${presentation.badgeClass}`}
                      >
                        {presentation.badge}
                      </span>
                      <span className="flex items-center gap-1.5 min-w-0">
                        <span
                          className="h-1.5 w-1.5 shrink-0 rounded-full shadow-sm"
                          style={{ backgroundColor: agent?.color ?? '#e4e4e7' }}
                          aria-hidden
                        />
                        <span className="truncate text-[10px] font-black uppercase tracking-widest text-darkDelegation">
                          {agentLabel}
                        </span>
                      </span>
                    </div>

                    <div className="flex shrink-0 items-center gap-0.5 -mr-1 -mt-0.5">
                      <span className="text-[9px] font-medium text-zinc-400 font-mono tabular-nums pr-1">
                        {formatLogTimeShort(entry.timestamp)}
                      </span>
                      <CopyButton text={copyText} title="Copy log entry" />
                    </div>
                  </div>

                  <p className="text-[12px] text-zinc-700 leading-relaxed font-medium [overflow-wrap:anywhere] pl-0.5">
                    {entry.action}
                  </p>

                  {entry.taskId && (
                    <p className="text-[9px] font-mono text-zinc-400 truncate" title={entry.taskId}>
                      Task {entry.taskId}
                    </p>
                  )}
                </div>
              </div>
            </article>
          </li>
        )
      })}
    </ul>
  )
}
