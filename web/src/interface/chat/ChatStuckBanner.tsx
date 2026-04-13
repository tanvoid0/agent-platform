import React from 'react'
import { Button } from '@/components/ui/button'
import { useUiStore } from '../../integration/store/uiStore'

export const ChatStuckBanner: React.FC<{
  visible: boolean
  isThinking: boolean
  onDismissStuck: () => void
  onResetRetry: () => void
}> = ({ visible, isThinking, onDismissStuck, onResetRetry }) => {
  if (!visible || !isThinking) return null
  return (
    <div className="mx-2 mb-1 rounded-xl border border-amber-200 bg-amber-50/90 px-3 py-2.5">
      <p className="text-[11px] font-semibold text-amber-950">
        Still waiting for a reply? The model may be stuck or unreachable.
      </p>
      <div className="mt-2 flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-8 rounded-lg border-amber-300 bg-white text-[10px] font-black uppercase tracking-wider text-amber-900"
          onClick={() => {
            useUiStore.getState().setThinking(false)
            onDismissStuck()
          }}
        >
          Stop waiting
        </Button>
        <Button
          type="button"
          size="sm"
          className="h-8 rounded-lg bg-amber-700 text-[10px] font-black uppercase tracking-wider text-white hover:bg-amber-800"
          onClick={() => void onResetRetry()}
        >
          Reset &amp; retry
        </Button>
      </div>
    </div>
  )
}
