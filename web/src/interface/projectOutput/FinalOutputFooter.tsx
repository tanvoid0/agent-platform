import { Button } from '@/components/ui/button'
import type { FinalAssetOutputType } from '../../integration/store/coreStoreTypes'

export type FinalOutputFooterProps = {
  referenceImages: string[]
  finalAssetType: FinalAssetOutputType
  copied: boolean
  onCopy: () => void | Promise<void>
}

export function FinalOutputFooter({
  referenceImages,
  finalAssetType,
  copied,
  onCopy,
}: FinalOutputFooterProps) {
  return (
    <div className="flex flex-col gap-6 border-t border-black/5 bg-white px-8 py-6">
      {referenceImages.length > 0 && (
        <div className="space-y-3">
          <p className="text-[9px] font-black uppercase tracking-widest text-zinc-300">Visual Inspiration</p>
          <div className="flex gap-2">
            {referenceImages.map((img, idx) => (
              <div key={idx} className="h-12 w-12 overflow-hidden rounded-lg border border-black/5 bg-zinc-50 shadow-sm">
                <img src={img} alt="Ref" className="h-full w-full object-cover opacity-50 grayscale" />
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="flex items-center justify-between">
        <div className="text-[9px] font-bold uppercase leading-none tracking-widest text-zinc-300">Generated March 2026</div>
        <Button
          type="button"
          onClick={() => void onCopy()}
          className="rounded-2xl bg-darkDelegation px-6 py-3 text-[10px] font-black uppercase tracking-[0.15em] text-white shadow-lg shadow-black/10 hover:bg-black active:scale-[0.98]"
        >
          {copied ? 'Copied!' : `Copy ${finalAssetType === 'text' ? 'Output' : 'Prompt'}`}
        </Button>
      </div>
    </div>
  )
}
