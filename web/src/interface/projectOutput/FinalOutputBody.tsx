import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Loader2, Download } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { isMockMediaSentinel } from '../../core/llm/mockDeliverables'
import type { FinalAssetOutputType } from '../../integration/store/coreStoreTypes'

export type FinalOutputBodyProps = {
  finalOutput: string
  finalAssetType: FinalAssetOutputType
  finalAssetContent: string
  isGeneratingAsset: boolean
}

function downloadFinalAsset(
  finalAssetType: FinalAssetOutputType,
  finalAssetContent: string,
): void {
  if (!finalAssetContent) return
  const link = document.createElement('a')
  if (finalAssetType === 'image') {
    link.href = `data:image/png;base64,${finalAssetContent}`
    link.download = `agentic-image-${Date.now()}.png`
  } else if (finalAssetType === 'audio') {
    link.href = `data:audio/mp3;base64,${finalAssetContent}`
    link.download = `agentic-audio-${Date.now()}.mp3`
  } else if (finalAssetType === 'video') {
    link.href = finalAssetContent
    link.download = `agentic-video-${Date.now()}.mp4`
    link.target = '_blank'
  }
  link.click()
}

export function FinalOutputBody({
  finalOutput,
  finalAssetType,
  finalAssetContent,
  isGeneratingAsset,
}: FinalOutputBodyProps) {
  if (isGeneratingAsset) {
    return (
      <div className="flex flex-col items-center justify-center gap-4 py-20">
        <Loader2 className="animate-spin text-zinc-300" size={40} strokeWidth={1.5} />
        <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">
          Generating {finalAssetType} asset...
        </p>
      </div>
    )
  }

  if (finalAssetType === 'text') {
    return (
      <div className="space-y-8">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{finalOutput || ''}</ReactMarkdown>
      </div>
    )
  }

  if (finalAssetType === 'image' && finalAssetContent) {
    return (
      <div className="space-y-4">
        <div className="group relative">
          <img
            src={`data:image/png;base64,${finalAssetContent}`}
            alt="Final Generated Asset"
            className="w-full rounded-2xl border border-black/5 shadow-xl"
          />
          <Button
            type="button"
            variant="secondary"
            size="icon-sm"
            onClick={() => downloadFinalAsset('image', finalAssetContent)}
            className="absolute top-4 right-4 border border-black/5 bg-white/90 opacity-0 shadow-lg backdrop-blur-sm transition-opacity hover:bg-white group-hover:opacity-100"
            title="Download Image"
          >
            <Download size={18} />
          </Button>
        </div>
        <div className="rounded-xl border border-zinc-100/50 bg-zinc-100/50 p-4">
          <p className="mb-1 text-[10px] font-black uppercase tracking-wider text-zinc-400">PROMPT USED:</p>
          <p className="text-xs italic leading-relaxed text-zinc-600">{finalOutput || 'No prompt metadata available.'}</p>
        </div>
      </div>
    )
  }

  if (finalAssetType === 'audio' && finalAssetContent && isMockMediaSentinel(finalAssetContent)) {
    return (
      <div className="space-y-3 rounded-2xl border border-dashed border-violet-200 bg-violet-50/40 p-8 text-center">
        <p className="text-[10px] font-black uppercase tracking-widest text-violet-600">Mock audio deliverable</p>
        <p className="text-sm leading-relaxed text-zinc-600">
          No audio was generated. Configure backend Gemini credentials to produce real output, or keep using this
          placeholder for local testing without spending tokens.
        </p>
      </div>
    )
  }

  if (finalAssetType === 'audio' && finalAssetContent) {
    return (
      <div className="space-y-4">
        <div className="flex flex-col items-center gap-3 rounded-2xl border border-zinc-100 bg-white p-3 shadow-sm sm:flex-row">
          <audio controls className="h-9 flex-1">
            <source src={`data:audio/mp3;base64,${finalAssetContent}`} type="audio/mp3" />
            Your browser does not support the audio element.
          </audio>
          <Button
            type="button"
            onClick={() => downloadFinalAsset('audio', finalAssetContent)}
            className="flex shrink-0 items-center justify-center gap-2 rounded-xl bg-darkDelegation px-4 py-2 text-[10px] font-black uppercase tracking-widest text-white hover:bg-black active:scale-95"
          >
            <Download size={14} strokeWidth={2.5} />
            Download Audio
          </Button>
        </div>
        <div className="rounded-xl border border-zinc-100/50 bg-zinc-100/50 p-4">
          <p className="mb-1 text-[10px] font-black uppercase tracking-wider text-zinc-400">LYRICS / PROMPT:</p>
          <p className="text-xs italic leading-relaxed text-zinc-500">{finalOutput || 'No prompt metadata available.'}</p>
        </div>
      </div>
    )
  }

  if (finalAssetType === 'video' && finalAssetContent && isMockMediaSentinel(finalAssetContent)) {
    return (
      <div className="space-y-3 rounded-2xl border border-dashed border-sky-200 bg-sky-50/40 p-8 text-center">
        <p className="text-[10px] font-black uppercase tracking-widest text-sky-600">Mock video deliverable</p>
        <p className="text-sm leading-relaxed text-zinc-600">
          No video was generated. Configure Gemini for cloud generation when you need real footage, or continue testing the
          agent flow without API cost.
        </p>
      </div>
    )
  }

  if (finalAssetType === 'video' && finalAssetContent) {
    return (
      <div className="space-y-4">
        <div className="group relative">
          <video controls className="w-full rounded-2xl border border-black/5 shadow-xl">
            <source src={finalAssetContent} type="video/mp4" />
            Your browser does not support the video tag.
          </video>
          <Button
            type="button"
            variant="secondary"
            size="icon-sm"
            onClick={() => downloadFinalAsset('video', finalAssetContent)}
            className="absolute top-4 right-4 z-10 border border-black/5 bg-white/90 opacity-0 shadow-lg backdrop-blur-sm transition-opacity hover:bg-white group-hover:opacity-100"
            title="Download Video"
          >
            <Download size={18} />
          </Button>
        </div>
        <div className="rounded-xl border border-zinc-100/50 bg-zinc-100/50 p-4">
          <p className="mb-1 text-[10px] font-black uppercase tracking-wider text-zinc-400">SCRIPT / PROMPT:</p>
          <p className="text-xs italic leading-relaxed text-zinc-600">{finalOutput || 'No prompt metadata available.'}</p>
        </div>
      </div>
    )
  }

  return null
}
