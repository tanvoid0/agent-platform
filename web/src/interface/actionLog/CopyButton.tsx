import { Check, Copy } from 'lucide-react'
import React, { useState } from 'react'
import { Button } from '@/components/ui/button'

export const CopyButton: React.FC<{ text: string; title?: string }> = ({
  text,
  title = 'Copy to clipboard',
}) => {
  const [copied, setCopied] = useState(false)

  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation()
    navigator.clipboard.writeText(text)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-xs"
      onClick={handleCopy}
      className={copied ? 'text-emerald-500' : 'text-zinc-300 hover:bg-zinc-100 hover:text-zinc-600'}
      title={title}
    >
      {copied ? <Check size={10} /> : <Copy size={10} />}
    </Button>
  )
}
