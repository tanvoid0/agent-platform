import { useCallback, useMemo, useState } from "react";

import { ApiError, postChatCompletion } from "../../api/client";
import type { ProcessRecord, SubagentNode, TaskNodeRecord } from "../../api/types";
import {
  buildProcessScopeSystemMessage,
  buildSubagentScopeSystemMessage,
} from "../../lib/chatContext";
import { useProcessChatState } from "../../hooks/useProcessChatState";
import { ChatComposer } from "./ChatComposer";
import { ChatMessageList } from "./ChatMessageList";
import { Button } from "@/components/ui/button";

type Props = {
  processId: number;
  process: ProcessRecord;
  /** Process-wide chat vs focused subagent. */
  mode: "process" | "subagent";
  /** Required when mode is subagent. */
  clientUuid?: string | null;
  subagent?: SubagentNode | null;
  task?: TaskNodeRecord | null;
};

const defaultModel = (): string | null => {
  const m = import.meta.env.VITE_DEFAULT_CHAT_MODEL as string | undefined;
  if (typeof m === "string" && m.trim() !== "") return m.trim();
  return null;
};

export function ProcessChatPanel({ processId, process, mode, clientUuid, subagent, task }: Props) {
  const scopeKey = mode === "process" ? "process" : `subagent:${clientUuid ?? ""}`;
  const { messages, setMessages, clear } = useProcessChatState(processId, scopeKey);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const model = useMemo(() => defaultModel(), []);

  const systemForRequest = useCallback(() => {
    if (mode === "process") {
      return buildProcessScopeSystemMessage(process);
    }
    if (!clientUuid) {
      return buildProcessScopeSystemMessage(process);
    }
    return buildSubagentScopeSystemMessage(process, clientUuid, subagent ?? undefined, task ?? undefined);
  }, [mode, process, clientUuid, subagent, task]);

  const onSend = useCallback(
    async (text: string) => {
      setError(null);
      const system = systemForRequest();
      const prior = messages.map((m) => ({ role: m.role, content: m.content }));
      const apiMessages = [system, ...prior, { role: "user" as const, content: text }];
      setMessages((prev) => [...prev, { role: "user", content: text }]);
      setPending(true);
      try {
        const { content } = await postChatCompletion({
          messages: apiMessages,
          model,
        });
        setMessages((prev) => [...prev, { role: "assistant", content: content || "(empty response)" }]);
      } catch (e) {
        const msg =
          e instanceof ApiError
            ? `${e.message}${e.status === 503 ? " — Is ORCHESTRATOR_MASTER_KEY set on the agent-platform server?" : ""}`
            : e instanceof Error
              ? e.message
              : String(e);
        setError(msg);
        setMessages((prev) => prev.slice(0, -1));
      } finally {
        setPending(false);
      }
    },
    [messages, model, setMessages, systemForRequest],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border px-3 py-2">
        <p className="text-muted-foreground text-xs leading-snug">
          {mode === "process"
            ? "Ask about this run (goal, status, tasks)."
            : "Ask about this subagent task and its output."}
        </p>
        <Button type="button" variant="outline" size="sm" className="shrink-0" onClick={clear}>
          Clear
        </Button>
      </div>
      {error ? (
        <div className="bg-destructive/10 text-destructive shrink-0 px-3 py-2 text-xs" role="alert">
          {error}
        </div>
      ) : null}
      <ChatMessageList messages={messages} pending={pending} />
      <ChatComposer onSend={onSend} disabled={pending} placeholder="Ask… (Enter to send, Shift+Enter for newline)" />
    </div>
  );
}
