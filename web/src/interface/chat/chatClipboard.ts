import type { LLMMessage } from '../../core/llm/types'

/** Plain-text transcript of visible chat messages for clipboard export. */
export function formatChatTranscript(messages: LLMMessage[], agentLabel: string): string {
  const parts: string[] = []
  for (const msg of messages) {
    if (msg.metadata?.internal) continue
    const label = msg.role === 'user' ? 'You' : agentLabel
    parts.push(`${label}:\n${msg.content.trim()}`)
  }
  return parts.join('\n\n')
}
