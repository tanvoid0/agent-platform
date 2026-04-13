import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Download,
  MessageSquare,
  Terminal,
  Zap,
} from 'lucide-react'
import React, { useState } from 'react'
import { Button } from '@/components/ui/button'
import { getAllAgents } from '../../data/agents'
import type { LLMToolCall } from '../../core/llm/types'
import type { DebugLogEntry } from '../../integration/store/coreStore'
import { useActiveTeam } from '../../integration/store/teamStore'
import { USER_COLOR, USER_COLOR_LIGHT } from '../../theme/brand'
import { formatTokens } from '../formatTokens'
import type { RequestPairingValue } from './buildRequestResponsePairing'
import { formatResponseDuration } from './buildRequestResponsePairing'
import { CopyButton } from './CopyButton'
import { formatLogTimeShort } from './formatLogTime'

export const DebugLogEntryView: React.FC<{
  entry: DebugLogEntry
  /** Request rows only: null = waiting; object = paired with latency (technical log). */
  requestPairing?: RequestPairingValue
}> = ({ entry, requestPairing }) => {
  const [isOpen, setIsOpen] = useState(false)
  const activeTeam = useActiveTeam()
  const agents = getAllAgents(activeTeam)
  const agent =
    entry.agentIndex === -1
      ? { name: 'System', color: '#71717a' }
      : agents.find((a) => a.index === entry.agentIndex)

  const totalTools = entry.phase === 'request' ? (entry.systemTools?.length ?? 0) : 0

  const pairingLine =
    entry.phase === 'request' && requestPairing !== undefined
      ? `RESPONSE_TIME: ${
          requestPairing === null
            ? 'pending'
            : formatResponseDuration(requestPairing.responseTimeMs)
        }\n`
      : ''

  const fullContent = `
AGENT: ${agent?.name} (${entry.phase})
TIME: ${formatLogTimeShort(entry.timestamp)}
PHASE: ${entry.phase}
${pairingLine}
${entry.phase === 'request'
  ? `
SYSTEM INSTRUCTION:
${entry.systemInstruction || 'None'}

USER BRIEF / MESSAGES:
${JSON.stringify(entry.contents, null, 2)}
`
  : `
CONTENT:
${entry.content || 'None'}

RAW RESPONSE:
${JSON.stringify(entry.raw, null, 2)}
`}
    `.trim()

  return (
    <div className="border-b border-zinc-50 last:border-0 py-3 group">
      <div className="flex items-center gap-1 mb-1 pr-1">
        <Button
          type="button"
          variant="ghost"
          onClick={() => setIsOpen(!isOpen)}
          className="h-auto flex-1 justify-between rounded p-1 text-left font-normal hover:bg-zinc-50/50"
        >
          <div className="flex flex-col gap-1.5 w-full">
            <div className="flex items-center justify-between w-full overflow-hidden">
              <div className="flex flex-col gap-1 overflow-hidden">
                <div className="flex items-center gap-2">
                  <div
                    className="w-2 h-2 rounded-full shrink-0"
                    style={{ backgroundColor: agent?.color ?? '#ccc' }}
                  />
                  <span className="text-[10px] font-black text-darkDelegation uppercase tracking-widest leading-none truncate">
                    {agent?.name}
                  </span>
                </div>
                <div className="flex items-center gap-1.5 ml-4">
                  <span
                    className="text-[8px] font-bold px-1.5 py-0.5 rounded uppercase tracking-tighter whitespace-nowrap"
                    style={
                      entry.phase === 'request'
                        ? {
                            backgroundColor: USER_COLOR_LIGHT,
                            color: USER_COLOR,
                          }
                        : {
                            backgroundColor: '#ecfdf5',
                            color: '#059669',
                          }
                    }
                  >
                    {entry.phase}
                  </span>
                  {entry.phase === 'request' && requestPairing !== undefined && (
                    <>
                      <span
                        className={`inline-flex items-center justify-center rounded-full p-0.5 border ${
                          requestPairing
                            ? 'bg-emerald-50 border-emerald-200 text-emerald-600'
                            : 'bg-amber-50 border-amber-200 text-amber-600'
                        }`}
                        title={
                          requestPairing
                            ? `Response received in ${formatResponseDuration(requestPairing.responseTimeMs)}`
                            : 'Waiting for response'
                        }
                      >
                        {requestPairing ? (
                          <CheckCircle2 className="size-3.5 shrink-0" strokeWidth={2.25} />
                        ) : (
                          <AlertTriangle className="size-3.5 shrink-0" strokeWidth={2.25} />
                        )}
                      </span>
                      {requestPairing && (
                        <span className="text-[8px] font-mono font-bold text-emerald-700 bg-emerald-50/80 border border-emerald-100 px-1 py-0.5 rounded leading-none whitespace-nowrap tabular-nums">
                          {formatResponseDuration(requestPairing.responseTimeMs)}
                        </span>
                      )}
                    </>
                  )}
                  {entry.phase === 'response' && entry.usage && (
                    <span className="text-[8px] font-mono font-bold text-zinc-400 bg-zinc-50 border border-zinc-100 px-1 py-0.5 rounded leading-none whitespace-nowrap">
                      T: {formatTokens(entry.usage.totalTokens)}
                    </span>
                  )}
                </div>
              </div>
              <div className="flex items-center gap-2 shrink-0 self-start mt-0.5">
                <span className="text-[8px] font-mono text-zinc-400">
                  {formatLogTimeShort(entry.timestamp)}
                </span>
                {isOpen ? (
                  <ChevronDown size={12} className="text-zinc-300" />
                ) : (
                  <ChevronRight size={12} className="text-zinc-300" />
                )}
              </div>
            </div>

            {entry.phase === 'response' &&
              entry.tool_calls &&
              entry.tool_calls.length > 0 &&
              !isOpen && (
                <div className="flex flex-wrap gap-1 pl-4">
                  {entry.tool_calls.map((tc, i) => (
                    <span
                      key={`${tc.id ?? tc.function?.name ?? 'tool'}-${i}`}
                      className="flex items-center gap-1 text-[8px] font-bold text-emerald-700 bg-emerald-50 border border-emerald-100 px-1.5 py-0.5 rounded shadow-sm"
                    >
                      <Zap size={8} />
                      {tc.function?.name || '(unknown)'}
                    </span>
                  ))}
                </div>
              )}
          </div>
        </Button>
        <div className="opacity-0 group-hover:opacity-100 transition-opacity">
          <CopyButton text={fullContent} />
        </div>
      </div>

      {isOpen && (
        <div className="mt-2 space-y-2 pl-4 border-l border-zinc-100">
          {entry.phase === 'request' ? (
            <>
              {entry.systemInstruction && (
                <details className="group/sp">
                  <summary className="flex items-center justify-between gap-1.5 py-1 cursor-pointer list-none">
                    <div className="flex items-center gap-1.5 opacity-50 hover:opacity-100 transition-opacity">
                      <ChevronRight
                        size={10}
                        className="text-zinc-400 group-open/sp:rotate-90 transition-transform"
                      />
                      <Terminal size={10} />
                      <span className="text-[9px] font-bold uppercase tracking-widest text-zinc-500">
                        System Instruction
                      </span>
                    </div>
                    <div onClick={(e) => e.stopPropagation()}>
                      <CopyButton text={entry.systemInstruction} />
                    </div>
                  </summary>
                  <pre className="mt-1.5 text-[10px] bg-zinc-50 p-2 rounded leading-relaxed text-zinc-600 whitespace-pre-wrap font-mono border border-zinc-100/50">
                    {entry.systemInstruction}
                  </pre>
                </details>
              )}

              <details className={`group/tools ${totalTools === 0 ? 'pointer-events-none' : ''}`}>
                <summary className="flex items-center justify-between gap-1.5 py-1 cursor-pointer list-none">
                  <div
                    className={`flex items-center gap-1.5 ${totalTools === 0 ? 'opacity-20' : 'opacity-50 hover:opacity-100 transition-opacity'}`}
                  >
                    <ChevronRight
                      size={10}
                      className={`text-zinc-400 group-open/tools:rotate-90 transition-transform ${totalTools === 0 ? 'invisible' : ''}`}
                    />
                    <Zap size={10} />
                    <span className="text-[9px] font-bold uppercase tracking-widest text-zinc-500">
                      System Tools ({totalTools})
                    </span>
                  </div>
                  {totalTools > 0 && (
                    <div onClick={(e) => e.stopPropagation()}>
                      <CopyButton text={JSON.stringify(entry.systemTools, null, 2)} />
                    </div>
                  )}
                </summary>
                {totalTools > 0 && (
                  <div className="mt-1.5 flex flex-wrap gap-1 bg-emerald-50/20 p-2 rounded border border-emerald-100/20">
                    {entry.systemTools.map((def, i) => (
                      <span
                        key={`${def.function.name}-${i}`}
                        className="text-[9px] font-mono font-bold text-emerald-700 bg-emerald-50 px-1.5 py-0.5 rounded border border-emerald-100/50 shadow-xs"
                      >
                        {def.function.name}
                      </span>
                    ))}
                  </div>
                )}
              </details>

              {entry.contents && entry.contents.length > 0 && (
                <details className="group/msgs" open>
                  <summary className="flex items-center justify-between gap-1.5 py-1 cursor-pointer list-none">
                    <div className="flex items-center gap-1.5 opacity-50 hover:opacity-100 transition-opacity">
                      <ChevronRight
                        size={10}
                        className="text-zinc-400 group-open/msgs:rotate-90 transition-transform"
                      />
                      <MessageSquare size={10} />
                      <span className="text-[9px] font-bold uppercase tracking-widest text-zinc-500">
                        Contents / Messages ({entry.contents.length})
                      </span>
                    </div>
                    <div onClick={(e) => e.stopPropagation()}>
                      <CopyButton text={JSON.stringify(entry.contents, null, 2)} />
                    </div>
                  </summary>
                  <div className="mt-2 space-y-2 max-h-80 overflow-y-auto pr-1 customize-scrollbar border-l-2 border-zinc-50 pl-2">
                    {entry.contents.map((m, i) => (
                      <div
                        key={`${m.role}-${m.content?.slice(0, 32) ?? 'empty'}-${i}`}
                        className={`p-2 rounded border group/msg transition-all ${
                          m.role === 'user'
                            ? 'bg-white border-zinc-100'
                            : 'bg-emerald-50/20 border-emerald-100/30'
                        }`}
                      >
                        <div className="flex items-center justify-between mb-1">
                          <span
                            className={`text-[8px] font-black uppercase tracking-widest ${
                              m.role === 'user' ? 'text-zinc-400' : 'text-emerald-600'
                            }`}
                          >
                            {m.role}
                          </span>
                          <div className="opacity-0 group-hover/msg:opacity-100 transition-opacity">
                            <CopyButton text={JSON.stringify(m, null, 2)} />
                          </div>
                        </div>
                        <div className="mt-1.5 space-y-2">
                          {m.content && (
                            <div className="text-[10px] text-zinc-700 leading-relaxed font-sans whitespace-pre-wrap py-1">
                              {m.content}
                            </div>
                          )}

                          {m.tool_calls && m.tool_calls.length > 0 && (
                            <div className="space-y-2 mt-2">
                              {m.tool_calls.map((tc: LLMToolCall, idx: number) => (
                                <div
                                  key={`${tc.id ?? tc.function?.name ?? 'tool'}-${idx}`}
                                  className="bg-darkDelegation rounded-lg overflow-hidden border border-darkDelegation shadow-lg"
                                >
                                  <div className="bg-darkDelegation px-2.5 py-1.5 flex items-center justify-between">
                                    <span className="text-[9px] font-black text-emerald-400 font-mono tracking-wider">
                                      {tc.function?.name}
                                    </span>
                                    <span className="text-[8px] font-bold text-zinc-500 uppercase tracking-tighter">
                                      Call
                                    </span>
                                  </div>
                                  <div className="p-2.5 bg-darkDelegation/50">
                                    <pre className="text-[9px] text-zinc-300 font-mono wrap-break-word whitespace-pre-wrap">
                                      {tc.function?.arguments}
                                    </pre>
                                  </div>
                                </div>
                              ))}
                            </div>
                          )}
                        </div>
                      </div>
                    ))}
                  </div>
                </details>
              )}
            </>
          ) : (
            <>
              <div className="pt-2">
                <div className="flex items-center justify-between gap-1.5 mb-1.5 opacity-50">
                  <div className="flex items-center gap-1.5">
                    <MessageSquare size={10} />
                    <span className="text-[9px] font-bold uppercase tracking-widest text-zinc-500">
                      Response Details
                    </span>
                  </div>
                  <CopyButton text={entry.content || ''} />
                </div>
                <div className="space-y-3">
                  {entry.content && (
                    <div className="text-[11px] bg-white p-3 rounded leading-relaxed text-zinc-700 border border-zinc-100 shadow-sm relative italic whitespace-pre-wrap">
                      <div className="absolute -top-2 left-2 bg-white px-1 text-[8px] font-black uppercase text-zinc-400 border border-zinc-100 rounded">
                        Text
                      </div>
                      {entry.content}
                    </div>
                  )}

                  {entry.tool_calls && entry.tool_calls.length > 0 && (
                    <div className="space-y-2">
                      <div className="flex items-center gap-1.5 ml-1">
                        <Zap size={10} className="text-emerald-500" />
                        <span className="text-[9px] font-black uppercase tracking-widest text-emerald-600">
                          Tool calls
                        </span>
                      </div>
                      {entry.tool_calls.map((tc, i) => {
                        const name = tc.function?.name || '(unknown)'
                        let args: Record<string, unknown> | null = null
                        try {
                          args = JSON.parse(tc.function?.arguments ?? '{}') as Record<string, unknown>
                        } catch {
                          args = null
                        }
                        return (
                          <div
                            key={`${tc.id ?? tc.function?.name ?? 'tool'}-${i}`}
                            className="bg-darkDelegation rounded-lg overflow-hidden border border-darkDelegation shadow-lg"
                          >
                            <div className="bg-darkDelegation px-2.5 py-1.5 flex items-center justify-between">
                              <span className="text-[10px] font-black text-emerald-400 font-mono tracking-wider">
                                {name}
                              </span>
                              <span className="text-[8px] font-bold text-zinc-500 uppercase tracking-tighter">
                                Arguments
                              </span>
                            </div>
                            <div className="p-2.5 bg-darkDelegation/50">
                              {args && Object.keys(args).length > 0 ? (
                                <div className="space-y-1.5">
                                  {Object.entries(args).map(([key, value]) => (
                                    <div key={key} className="flex flex-col gap-0.5">
                                      <span className="text-[8px] font-bold text-zinc-500 uppercase tracking-tighter">
                                        {key}
                                      </span>
                                      <div className="text-[9px] text-zinc-300 font-mono bg-darkDelegation/50 p-1.5 rounded border border-zinc-700/50 wrap-break-word whitespace-pre-wrap">
                                        {typeof value === 'object'
                                          ? JSON.stringify(value, null, 2)
                                          : String(value)}
                                      </div>
                                    </div>
                                  ))}
                                </div>
                              ) : (
                                <span className="text-[9px] text-zinc-500 italic">No arguments</span>
                              )}
                            </div>
                          </div>
                        )
                      })}
                    </div>
                  )}
                </div>
              </div>

              {entry.raw && (
                <details className="group/raw">
                  <summary className="flex items-center justify-between gap-1.5 py-1 cursor-pointer list-none">
                    <div className="flex items-center gap-1.5 opacity-50 hover:opacity-100 transition-opacity">
                      <ChevronRight
                        size={10}
                        className="text-zinc-400 group-open/raw:rotate-90 transition-transform"
                      />
                      <Download size={10} />
                      <span className="text-[9px] font-bold uppercase tracking-widest text-zinc-500">
                        Raw LLM Response
                      </span>
                    </div>
                    <div onClick={(e) => e.stopPropagation()}>
                      <CopyButton text={JSON.stringify(entry.raw, null, 2)} />
                    </div>
                  </summary>
                  <pre className="mt-1.5 text-[9px] bg-darkDelegation text-zinc-400 p-2 rounded leading-relaxed whitespace-pre overflow-x-auto font-mono border border-darkDelegation">
                    {JSON.stringify(entry.raw, null, 2)}
                  </pre>
                </details>
              )}
            </>
          )}
        </div>
      )}
    </div>
  )
}
