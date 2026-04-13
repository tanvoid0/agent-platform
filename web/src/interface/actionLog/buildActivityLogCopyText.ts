import type { ActionLogEntry } from '../../integration/store/coreStore'
import { formatLogTimeShort } from './formatLogTime'

export function buildActivityLogCopyText(
  entry: ActionLogEntry,
  agentLabel: string,
): string {
  const time = formatLogTimeShort(entry.timestamp)
  const taskLine = entry.taskId ? `\nTask: ${entry.taskId}` : ''
  return `${time} · ${agentLabel}\n${entry.action}${taskLine}`
}
