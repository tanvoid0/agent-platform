import { useEffect, useRef } from "react";

import type { StoredChatMessage } from "../../hooks/useProcessChatState";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

type Props = {
  messages: StoredChatMessage[];
  pending?: boolean;
  className?: string;
};

export function ChatMessageList({ messages, pending, className }: Props) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [messages, pending]);

  return (
    <ScrollArea className={cn("min-h-0 flex-1", className)}>
      <div className="flex flex-col gap-3 p-3">
        {messages.length === 0 && !pending ? (
          <p className="text-muted-foreground text-xs leading-relaxed">
            Ask about this process or the selected task. Messages stay in this browser session only.
          </p>
        ) : null}
        {messages.map((m, i) => (
          <div
            key={`${m.role}-${i}`}
            className={cn(
              "max-w-[95%] rounded-lg px-3 py-2 text-sm leading-snug break-words whitespace-pre-wrap",
              m.role === "user"
                ? "ml-auto bg-primary text-primary-foreground"
                : "mr-auto border border-border bg-muted/50 text-foreground",
            )}
          >
            {m.content}
          </div>
        ))}
        {pending ? (
          <div className="text-muted-foreground mr-auto text-xs italic">Thinking…</div>
        ) : null}
        <div ref={bottomRef} className="h-px shrink-0" aria-hidden />
      </div>
    </ScrollArea>
  );
}
