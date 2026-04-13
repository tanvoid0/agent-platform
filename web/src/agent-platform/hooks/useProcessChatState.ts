import { useCallback, useEffect, useState } from "react";

export type StoredChatRole = "user" | "assistant";

export interface StoredChatMessage {
  role: StoredChatRole;
  content: string;
}

const PREFIX = "agent-platform:chat:v1:";

function key(processId: number, scopeKey: string): string {
  return `${PREFIX}${processId}:${scopeKey}`;
}

function loadMessages(processId: number | null, scopeKey: string): StoredChatMessage[] {
  if (processId == null) return [];
  try {
    const raw = sessionStorage.getItem(key(processId, scopeKey));
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (x): x is StoredChatMessage =>
        x != null &&
        typeof x === "object" &&
        ((x as StoredChatMessage).role === "user" ||
          (x as StoredChatMessage).role === "assistant") &&
        typeof (x as StoredChatMessage).content === "string",
    );
  } catch {
    return [];
  }
}

function persistMessages(processId: number, scopeKey: string, messages: StoredChatMessage[]): void {
  try {
    sessionStorage.setItem(key(processId, scopeKey), JSON.stringify(messages));
  } catch {
    /* ignore */
  }
}

/**
 * Client-only chat history for POST /api/v1/chat, keyed by process and scope (`process` or `subagent:<uuid>`).
 */
export function useProcessChatState(processId: number | null, scopeKey: string) {
  const [messages, setMessages] = useState<StoredChatMessage[]>(() =>
    loadMessages(processId, scopeKey),
  );

  useEffect(() => {
    setMessages(loadMessages(processId, scopeKey));
  }, [processId, scopeKey]);

  useEffect(() => {
    if (processId == null) return;
    persistMessages(processId, scopeKey, messages);
  }, [processId, scopeKey, messages]);

  const clear = useCallback(() => {
    setMessages([]);
    if (processId != null) {
      try {
        sessionStorage.removeItem(key(processId, scopeKey));
      } catch {
        /* ignore */
      }
    }
  }, [processId, scopeKey]);

  return { messages, setMessages, clear };
}
