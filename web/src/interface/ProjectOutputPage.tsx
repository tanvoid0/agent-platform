import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { ArrowLeft } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useCoreStore } from '../integration/store/coreStore'
import { useActiveTeam } from '../integration/store/teamStore'
import { TeamOutputBadge } from './components/TeamOutputBadge'
import { FinalOutputBody } from './projectOutput/FinalOutputBody'
import { FinalOutputFooter } from './projectOutput/FinalOutputFooter'

export function ProjectOutputPage() {
  const navigate = useNavigate()
  const activeTeam = useActiveTeam()
  const {
    finalOutput,
    finalAssetType,
    finalAssetContent,
    isGeneratingAsset,
    referenceImages,
  } = useCoreStore()
  const [copied, setCopied] = useState(false)

  const handleCopy = async () => {
    await navigator.clipboard.writeText(finalOutput || '')
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  return (
    <div className="min-h-screen bg-zinc-50">
      <header className="sticky top-0 z-10 border-b border-black/5 bg-white/95 px-4 py-4 shadow-sm backdrop-blur supports-[backdrop-filter]:bg-white/80 sm:px-8">
        <div className="mx-auto flex max-w-4xl flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="gap-2 text-[10px] font-black uppercase tracking-widest"
              onClick={() => navigate('/')}
            >
              <ArrowLeft className="size-3.5" strokeWidth={2.5} />
              Back to office
            </Button>
            <div>
              <h1 className="text-lg font-black uppercase tracking-tight text-darkDelegation sm:text-xl">
                Project output
              </h1>
              <p className="text-[11px] text-zinc-500">Final deliverable from your team</p>
            </div>
            <TeamOutputBadge system={activeTeam} className="hidden sm:flex" />
          </div>
        </div>
      </header>

      <main className="mx-auto max-w-4xl px-4 py-8 sm:px-8">
        <div className="overflow-hidden rounded-[32px] border border-black/10 bg-white shadow-xl">
          <div className="border-b border-black/5 px-6 py-5 sm:px-8">
            <h2 className="flex flex-wrap items-center gap-2 text-sm font-black uppercase tracking-widest text-darkDelegation">
              {finalAssetType !== 'text' && (
                <span className="rounded-md bg-darkDelegation px-2 py-0.5 text-[8px] tracking-tighter text-white">
                  {(activeTeam?.outputType || finalAssetType).toUpperCase()}
                </span>
              )}
              Final {finalAssetType} deliverable
            </h2>
          </div>

          <div className="px-6 py-8 sm:px-8">
            <div className="markdown-content font-sans text-sm leading-relaxed text-zinc-700">
              <FinalOutputBody
                finalOutput={finalOutput || ''}
                finalAssetType={finalAssetType}
                finalAssetContent={finalAssetContent ?? ''}
                isGeneratingAsset={isGeneratingAsset}
              />
            </div>
          </div>

          <FinalOutputFooter
            referenceImages={referenceImages}
            finalAssetType={finalAssetType}
            copied={copied}
            onCopy={handleCopy}
          />
        </div>
      </main>
    </div>
  )
}
