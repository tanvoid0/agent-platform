import { Send } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { USER_COLOR, USER_COLOR_LIGHT } from '../../theme/brand'

export const ChatComposer: React.FC<{
  input: string
  onInputChange: (e: React.ChangeEvent<HTMLTextAreaElement>) => void
  onSend: () => void
  isThinking: boolean
  canSend: boolean
  sendBlockedReason?: string
  agentColor: string
  textareaRef: React.RefObject<HTMLTextAreaElement | null>
}> = ({ input, onInputChange, onSend, isThinking, canSend, sendBlockedReason, agentColor, textareaRef }) => (
  <div className="p-2 border-t border-zinc-50">
    <div className="relative flex items-center gap-2">
      <div className="flex-1 relative">
        <Textarea
          ref={textareaRef}
          value={input}
          onChange={onInputChange}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              if (canSend && !isThinking) onSend()
            }
          }}
          placeholder={canSend ? 'Message (↵ to send)' : sendBlockedReason ?? 'Unavailable'}
          disabled={!canSend}
          className="w-full resize-none rounded-2xl border border-zinc-200 bg-white px-3 py-3 pr-12 text-sm transition-all [scrollbar-width:none] focus-visible:ring-2"
          style={{
            borderColor: input.trim() ? USER_COLOR : undefined,
            boxShadow: input.trim() ? `0 0 0 2px ${USER_COLOR_LIGHT}` : undefined,
          }}
        />
      </div>
      <Button
        type="button"
        onClick={onSend}
        disabled={!input.trim() || isThinking || !canSend}
        title={!canSend ? sendBlockedReason : undefined}
        style={{
          backgroundColor: !input.trim() || isThinking || !canSend ? undefined : agentColor,
        }}
        className={`flex size-11 shrink-0 items-center justify-center rounded-2xl font-black text-xs uppercase tracking-widest active:scale-95 ${
          !input.trim() || isThinking || !canSend
            ? 'cursor-not-allowed bg-zinc-100 text-zinc-400'
            : 'text-white shadow-lg hover:brightness-90'
        }`}
      >
        <Send size={16} strokeWidth={3} />
      </Button>
    </div>
    <p className="text-[8px] text-zinc-400 mt-2 text-center font-medium uppercase tracking-wider">
      Shift + ↵ for new line
    </p>
  </div>
)
