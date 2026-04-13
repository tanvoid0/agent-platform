import { useState, useEffect } from 'react'
import { useCoreStore } from '../integration/store/coreStore'
import { useActiveTeam } from '../integration/store/teamStore'
import { useSceneManager } from '../simulation/SceneContext'
import {
  Sparkles,
  Settings2,
  Image as ImageIcon,
  Video,
  Music,
  Type,
  X,
  Check,
  Monitor,
  Clock,
  Maximize,
  Volume2,
  AlertCircle
} from 'lucide-react'
import { getMediaReadiness, getOutputModelPickerOptions } from '../core/llm/llmFacade'
import { resolveEffectiveGenerationModel } from '../core/llm/resolveGenerationModel'
import { useLlmSessionStore } from '../integration/store/llmSessionStore'
import type { OutputGenerationParams } from '../core/llm/outputGenerationParams'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { InfoBubble } from './components/InfoBubble'
import { ModalBackdrop, ModalRoot } from './components/ModalChrome'

export function OutputReviewModal() {
  const {
    isReviewingOutput,
    setReviewingOutput,
    pendingOutputPrompt,
    pendingOutputParams,
    resetProject,
    referenceImages,
    openMultimodalAssetBlocked,
  } = useCoreStore()

  const activeTeam = useActiveTeam()
  const llmConfig = useLlmSessionStore((s) => s.llmConfig)
  const scene = useSceneManager()
  const [prompt, setPrompt] = useState(pendingOutputPrompt)
  const [params, setParams] = useState<OutputGenerationParams>(pendingOutputParams)
  const [isConfirmingReset, setIsConfirmingReset] = useState(false)

  // Sync internal state when store changes
  useEffect(() => {
    if (isReviewingOutput) {
      setPrompt(pendingOutputPrompt)
      setParams(pendingOutputParams)
      setIsConfirmingReset(false)
    }
  }, [isReviewingOutput, pendingOutputPrompt, pendingOutputParams])

  if (!isReviewingOutput) return null

  const handleGenerate = async () => {
    const out = activeTeam?.outputType
    const needsCloud = out === 'image' || out === 'music' || out === 'video'
    if (needsCloud) {
      const media = getMediaReadiness(
        out as 'image' | 'music' | 'video',
        useLlmSessionStore.getState().llmConfig.apiKey
      )
      if (!media.ready) {
        openMultimodalAssetBlocked({
          summaryPrompt: prompt,
          outputType: out === 'music' ? 'music' : out,
          backend: media.backend,
          reason: media.reason,
        })
        setReviewingOutput(false)
        return
      }
    }
    const brain = scene?.getLeadBrain()
    if (brain) {
      await brain.processFinalAsset(prompt, params)
    }
  }

  const handleCancelAndReset = () => {
    setIsConfirmingReset(true)
  }

  const confirmReset = () => {
    resetProject()
    setIsConfirmingReset(false)
    setReviewingOutput(false)
  }

  function updateParam<K extends keyof OutputGenerationParams>(key: K, value: OutputGenerationParams[K]) {
    setParams((prev) => ({ ...prev, [key]: value }))
  }

  const renderImageControls = () => (
    <div className="grid grid-cols-2 gap-4">
      <div className="space-y-2">
        <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
          <Maximize size={12} /> Aspect Ratio
          <InfoBubble text="The horizontal or vertical proportions of the generated asset." />
        </label>
        <select
          value={params.aspectRatio || '16:9'}
          onChange={(e) => updateParam('aspectRatio', e.target.value)}
          className="w-full bg-white border border-zinc-200 rounded-xl px-3 py-2 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
        >
          <option value="1:1">1:1 Square</option>
          <option value="16:9">16:9 Cinematic</option>
          <option value="9:16">9:16 Vertical</option>
          <option value="4:3">4:3 Classic</option>
          <option value="3:2">3:2 Professional</option>
        </select>
      </div>
      <div className="space-y-2">
        <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
          <Settings2 size={12} /> Image Size
          <InfoBubble text="Target dimensions for the final image. Higher sizes offer more detail but may take longer." />
        </label>
        <select
          value={params.imageSize || '1K'}
          onChange={(e) => updateParam('imageSize', e.target.value)}
          className="w-full bg-white border border-zinc-200 rounded-xl px-3 py-2 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
        >
          <option value="512">512px (Fast)</option>
          <option value="1K">1K (Standard)</option>
          <option value="2K">2K (High Res)</option>
          <option value="4K">4K (Ultra)</option>
        </select>
      </div>
    </div>
  )

  const renderVideoControls = () => (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-4">
        <div className="space-y-2">
          <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
            <Monitor size={12} /> Resolution
            <InfoBubble text="Video output quality. Higher resolutions increase visual fidelity and processing requirements." />
          </label>
          <select
            value={params.resolution || '720p'}
            onChange={(e) => updateParam('resolution', e.target.value)}
            className="w-full bg-white border border-zinc-200 rounded-xl px-3 py-2 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
          >
            <option value="720p">720p HD</option>
            <option value="1080p">1080p Full HD</option>
            <option value="4k">4K Vision</option>
          </select>
        </div>
        <div className="space-y-2">
          <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
            <Clock size={12} /> Duration
            <InfoBubble text="Total runtime of the generated video clip." />
          </label>
          <select
            value={params.durationSeconds || 4}
            onChange={(e) => updateParam('durationSeconds', parseInt(e.target.value))}
            className="w-full bg-white border border-zinc-200 rounded-xl px-3 py-2 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
          >
            <option value="4">4 Seconds</option>
            <option value="6">6 Seconds</option>
            <option value="8">8 Seconds</option>
          </select>
        </div>
      </div>
    </div>
  )

  const renderModelControl = () => {
    const out = activeTeam.outputType
    const pickerType = out === 'music' ? 'music' : out === 'video' ? 'video' : out === 'image' ? 'image' : 'text'
    const models = getOutputModelPickerOptions(pickerType)
    const effective = params.model || resolveEffectiveGenerationModel(llmConfig, activeTeam)

    return (
      <div className="space-y-2">
        <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
          <Sparkles size={12} /> Generation Model
          <InfoBubble text="Uses the same provider as agent chat and media routing (see model-config). Pick a listed model id for this backend." />
        </label>
        {models.length === 0 ? (
          <p className="text-xs text-amber-800 bg-amber-50 border border-amber-100 rounded-xl px-3 py-2">
            No models available for this output type (routing may be disabled in model-config).
          </p>
        ) : (
          <select
            value={effective}
            onChange={(e) => updateParam('model', e.target.value)}
            className="w-full bg-white border border-zinc-200 rounded-xl px-3 py-2 text-xs font-medium focus:ring-2 focus:ring-darkDelegation outline-none"
          >
            {!models.includes(effective) && (
              <option value={effective}>{effective} (current)</option>
            )}
            {models.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        )}
      </div>
    )
  }

  const Icon = {
    image: ImageIcon,
    video: Video,
    music: Music,
    text: Type
  }[activeTeam.outputType] || Sparkles

  return (
    <ModalRoot layer="modalElevated" paddingClassName="p-4">
      <ModalBackdrop tone="darkDelegation" onRequestClose={handleCancelAndReset} />
      <div
        className="relative bg-white border border-black/10 rounded-[32px] w-180 max-w-full max-h-[90vh] flex flex-col shadow-2xl overflow-hidden scale-in"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-8 py-6 border-b border-zinc-100 bg-white">
          <div className="flex items-center gap-4">
            <div className="w-12 h-12 rounded-2xl bg-darkDelegation flex items-center justify-center text-white shadow-lg">
              <Icon size={24} />
            </div>
            <div>
              <h2 className="text-sm font-black uppercase tracking-widest text-darkDelegation flex items-center gap-2">
                Review & Optimize Output
              </h2>
              <p className="text-[11px] text-zinc-400 mt-0.5">
                The lead agent has synthesized the team's work. Fine-tune it before final generation.
              </p>
            </div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={handleCancelAndReset}
            className="size-8 rounded-full text-zinc-400 hover:bg-zinc-100 hover:text-zinc-700 active:scale-90"
            aria-label="Close"
          >
            <X size={18} />
          </Button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto px-8 py-8 space-y-8 bg-zinc-50/30">
          {/* Prompt Editor */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400">
                PROMPT / CONTENT
              </label>
              <div className="px-2 py-0.5 bg-zinc-100 rounded text-[9px] font-bold text-zinc-400 tracking-tighter">
                EDITABLE
              </div>
            </div>
            <Textarea
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              className="h-40 resize-none rounded-2xl border-zinc-200 bg-white p-4 font-sans text-sm leading-relaxed text-zinc-700 shadow-sm focus-visible:ring-2 focus-visible:ring-darkDelegation"
              placeholder="Enter the final generation prompt..."
            />
          </div>

          {/* Parameters Grid */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-8">
            <div className="space-y-6">
              {renderModelControl()}
              {activeTeam.outputType === 'image' && renderImageControls()}
              {activeTeam.outputType === 'video' && renderVideoControls()}
            </div>

            <div className="bg-darkDelegation rounded-[24px] p-6 text-white space-y-4 shadow-xl">
              <h3 className="text-[10px] font-black uppercase tracking-[0.2em] text-zinc-500">System Information</h3>
              <div className="space-y-4">
                <div>
                  <p className="text-[9px] text-zinc-500 uppercase font-bold tracking-widest">Team</p>
                  <p className="text-xs font-black">{activeTeam.teamName}</p>
                </div>
                <div>
                  <p className="text-[9px] text-zinc-500 uppercase font-bold tracking-widest">Output Type</p>
                  <p className="text-xs font-black capitalize">{activeTeam.outputType}</p>
                </div>

                {referenceImages.length > 0 && (
                  <div className="pt-6 border-t border-white/10 space-y-3">
                    <p className="text-[9px] text-zinc-500 uppercase font-bold tracking-widest">Visual Inspiration</p>
                    <div className="grid grid-cols-3 gap-2">
                      {referenceImages.map((img, idx) => (
                        <div key={idx} className="aspect-square rounded-lg overflow-hidden border border-white/5 bg-white/5">
                          <img src={img} alt="Ref" className="w-full h-full object-cover opacity-80 hover:opacity-100 transition-opacity" />
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                <div className="pt-4 border-t border-white/10">
                  <p className="text-[10px] leading-relaxed text-zinc-400 italic">
                    "This is the final terminal phase. You can adjust the parameters and the synthesized prompt to get the best result. Once approved, the simulation will complete and your asset will be generated."
                  </p>
                </div>
              </div>
            </div>
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-3 border-t border-zinc-100 bg-white px-8 py-6">
          <Button
            type="button"
            variant="outline"
            onClick={handleCancelAndReset}
            className="rounded-2xl border-zinc-200 bg-white px-6 py-3 text-[10px] font-black uppercase tracking-[0.15em] text-zinc-500 hover:border-red-500 hover:bg-zinc-50 hover:text-red-500 active:scale-[0.98]"
          >
            Cancel & Reset Project
          </Button>

          <Button
            type="button"
            onClick={handleGenerate}
            className="flex items-center gap-2 rounded-2xl bg-darkDelegation px-8 py-3 text-[10px] font-black uppercase tracking-[0.15em] text-white shadow-lg shadow-black/10 hover:bg-black active:scale-[0.98]"
          >
            <Check size={14} strokeWidth={3} />
            Approve & Generate
          </Button>
        </div>
      </div>

      {/* Confirmation Modal Overlay */}
      {isConfirmingReset && (
        <ModalRoot layer="modalNested" paddingClassName="p-4" className="cursor-default">
          <ModalBackdrop
            tone="darkDelegation"
            onRequestClose={() => setIsConfirmingReset(false)}
          />
          <div
            className="relative bg-white border border-black/10 rounded-[24px] w-96 p-8 shadow-2xl flex flex-col items-center text-center gap-6 animate-in fade-in zoom-in-95 duration-200"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="w-16 h-16 rounded-2xl bg-red-50 flex items-center justify-center text-red-500">
              <AlertCircle size={32} />
            </div>
            <div>
              <h3 className="text-sm font-black uppercase tracking-widest text-darkDelegation">Are you absolutely sure?</h3>
              <p className="text-xs text-zinc-400 mt-2 leading-relaxed">
                All progress will be lost and the project will be reset to its initial state. This action cannot be undone.
              </p>
            </div>
            <div className="mt-2 flex w-full flex-col gap-2">
              <Button
                type="button"
                onClick={confirmReset}
                className="w-full rounded-2xl bg-red-500 py-4 text-[10px] font-black uppercase tracking-[0.15em] text-white hover:bg-red-600 active:scale-[0.98]"
              >
                Yes, Reset Project
              </Button>
              <Button
                type="button"
                variant="secondary"
                onClick={() => setIsConfirmingReset(false)}
                className="w-full rounded-2xl py-4 text-[10px] font-black uppercase tracking-[0.15em] text-zinc-500 hover:bg-zinc-200 active:scale-[0.98]"
              >
                No, Go Back
              </Button>
            </div>
          </div>
        </ModalRoot>
      )}
    </ModalRoot>
  )
}
