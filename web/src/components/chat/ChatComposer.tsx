import { useCallback, useState } from "react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const textareaClass = cn(
  "w-full min-w-0 rounded-lg border border-input bg-transparent px-2.5 py-2 text-sm transition-colors outline-none",
  "placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
  "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
  "dark:bg-input/30",
);

type Props = {
  onSend: (text: string) => void | Promise<void>;
  disabled?: boolean;
  placeholder?: string;
};

export function ChatComposer({ onSend, disabled, placeholder }: Props) {
  const [value, setValue] = useState("");

  const submit = useCallback(async () => {
    const t = value.trim();
    if (!t || disabled) return;
    setValue("");
    await onSend(t);
  }, [value, disabled, onSend]);

  return (
    <div className="border-border flex shrink-0 flex-col gap-2 border-t p-3">
      <textarea
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder={placeholder ?? "Message…"}
        disabled={disabled}
        rows={3}
        className={cn(textareaClass, "min-h-[4.5rem] resize-y")}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void submit();
          }
        }}
      />
      <Button type="button" size="sm" className="self-end" disabled={disabled || !value.trim()} onClick={() => void submit()}>
        Send
      </Button>
    </div>
  );
}
