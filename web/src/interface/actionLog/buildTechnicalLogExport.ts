import type { DebugLogEntry } from '../../integration/store/coreStore'

type AgentRef = { index: number; name: string }

function agentName(agents: AgentRef[], agentIndex: number): string {
  if (agentIndex === -1) return 'System'
  return agents.find((a) => a.index === agentIndex)?.name ?? 'Unknown'
}

/** Full export text for all technical debug entries (download .txt). */
export function buildTechnicalLogExportText(
  entries: DebugLogEntry[],
  agents: AgentRef[],
): string {
  return entries
    .map((entry) => {
      const name = agentName(agents, entry.agentIndex)
      return `
=========================================
AGENT: ${name} (${entry.phase})
TIME: ${new Date(entry.timestamp).toLocaleString()}
PHASE: ${entry.phase}
=========================================

${entry.phase === 'request'
  ? `
SYSTEM INSTRUCTION:
${entry.systemInstruction || 'None'}

USER BRIEF / MESSAGES:
${JSON.stringify(entry.contents, null, 2)}

SYSTEM TOOLS:
${JSON.stringify(entry.systemTools, null, 2)}
`
  : `
CONTENT:
${entry.content || 'None'}

TOOL CALLS:
${JSON.stringify(entry.tool_calls || [], null, 2)}

RAW RESPONSE:
${JSON.stringify(entry.raw, null, 2)}
`}
`.trim()
    })
    .join('\n\n\n')
}
