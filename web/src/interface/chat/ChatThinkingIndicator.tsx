import React from 'react'

export const ChatThinkingIndicator: React.FC = () => (
  <div className="flex items-start gap-3">
    <div className="w-4 h-4 text-zinc-300 animate-pulse mt-1">
      <svg viewBox="0 0 24 24" fill="currentColor">
        <path d="M12 2L14.85 9.15L22 12L14.85 14.85L12 22L9.15 14.85L2 12L9.15 9.15L12 2Z" />
      </svg>
    </div>
    <div className="bg-zinc-50 px-4 py-3 rounded-2xl rounded-tl-none">
      <div className="flex gap-1">
        <div
          className="w-1.5 h-1.5 bg-zinc-300 rounded-full animate-bounce"
          style={{ animationDelay: '0ms' }}
        />
        <div
          className="w-1.5 h-1.5 bg-zinc-300 rounded-full animate-bounce"
          style={{ animationDelay: '150ms' }}
        />
        <div
          className="w-1.5 h-1.5 bg-zinc-300 rounded-full animate-bounce"
          style={{ animationDelay: '300ms' }}
        />
      </div>
    </div>
  </div>
)
