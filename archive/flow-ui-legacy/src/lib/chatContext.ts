import type { ChatCompletionMessage, ProcessRecord, SubagentNode, TaskNodeRecord } from "../api/types";

const OUTPUT_SNIP_LEN = 3000;

export function buildProcessScopeSystemMessage(process: ProcessRecord): ChatCompletionMessage {
  const parts = [
    "You are a concise assistant for the agent-platform orchestration UI.",
    `Process id: ${process.id}`,
    `Status: ${process.status}`,
    `Goal: ${process.goal}`,
  ];
  if (process.failure_reason?.trim()) {
    parts.push(`Last failure: ${process.failure_reason.trim()}`);
  }
  parts.push("Help interpret DAG tasks, statuses, and next steps. Do not invent task outputs.");
  return { role: "system", content: parts.join("\n") };
}

export function buildSubagentScopeSystemMessage(
  process: ProcessRecord,
  clientUuid: string,
  sub: SubagentNode | null | undefined,
  task: TaskNodeRecord | null | undefined,
): ChatCompletionMessage {
  const parts = [
    "You are a concise assistant for a single subagent task in agent-platform.",
    `Process id: ${process.id}`,
    `Process status: ${process.status}`,
    `Process goal: ${process.goal}`,
    `Focused client_uuid: ${clientUuid}`,
  ];
  if (sub?.role) parts.push(`Role: ${sub.role}`);
  if (task) {
    parts.push(`Task status: ${task.status}`);
    if (task.output?.trim()) {
      const snip =
        task.output.length > OUTPUT_SNIP_LEN
          ? `${task.output.slice(0, OUTPUT_SNIP_LEN)}…`
          : task.output;
      parts.push(`Task output (snippet): ${snip}`);
    }
  }
  parts.push("Answer about this task and role. Do not invent outputs not shown above.");
  return { role: "system", content: parts.join("\n") };
}
