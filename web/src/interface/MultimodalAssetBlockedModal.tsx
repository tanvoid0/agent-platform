import { AlertTriangle, Check, Clock, Image as ImageIcon, Info, Music, Video } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useCoreStore } from '../integration/store/coreStore';
import { ModalRoot } from './components/ModalChrome';

const typeLabel = (t: 'image' | 'music' | 'video') =>
  t === 'music' ? 'audio' : t;

const TypeIcon = ({ t }: { t: 'image' | 'music' | 'video' }) => {
  if (t === 'image') return <ImageIcon size={18} strokeWidth={2} />;
  if (t === 'music') return <Music size={18} strokeWidth={2} />;
  return <Video size={18} strokeWidth={2} />;
};

export function MultimodalAssetBlockedModal() {
  const blocked = useCoreStore((s) => s.multimodalAssetBlocked);
  const resolveSkipped = useCoreStore((s) => s.resolveMultimodalAssetSkipped);
  const resolveMocked = useCoreStore((s) => s.resolveMultimodalAssetMocked);
  const resolveDeferred = useCoreStore((s) => s.resolveMultimodalAssetDeferred);

  if (!blocked) return null;

  const { outputType } = blocked;
  const label = typeLabel(outputType);
  const backendLabel =
    blocked.backend === 'gemini'
      ? 'Cloud'
      : blocked.backend === 'ollama'
        ? 'Server'
        : blocked.backend === 'disabled'
          ? 'Disabled'
          : 'Configured backend';

  return (
    <ModalRoot
      layer="modalElevated"
      paddingClassName="p-4"
      className="pointer-events-auto bg-white/70 backdrop-blur-xl"
    >
      <div
        className="w-full max-w-md bg-white border border-zinc-200 rounded-[28px] shadow-2xl overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-6 pt-6 pb-4 border-b border-zinc-100 flex items-start gap-3">
          <div className="p-2.5 rounded-2xl bg-amber-50 text-amber-600 shrink-0">
            <AlertTriangle size={22} strokeWidth={2} />
          </div>
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-black uppercase tracking-wider text-darkDelegation">
              Cannot generate final {label}
            </h2>
            <p className="text-xs text-zinc-500 mt-1.5 leading-relaxed">
              Your team delivered the work, but generation for this modality is currently unavailable on{' '}
              <strong className="text-zinc-700">{backendLabel}</strong>. Choose how to close this delivery
              (skip, mock, or defer).
            </p>
            {blocked.reason && (
              <p className="text-[11px] text-amber-700 mt-2 leading-relaxed">
                {blocked.reason}
              </p>
            )}
          </div>
        </div>

        <div className="px-6 py-4 bg-zinc-50/80 border-b border-zinc-100">
          <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-2 flex items-center gap-2">
            <TypeIcon t={outputType} />
            Requested output
          </p>
          <p className="text-xs font-mono text-zinc-600 lowercase">{outputType}</p>
        </div>

        <div className="space-y-3 px-6 py-5">
          <Button
            type="button"
            variant="outline"
            onClick={() => resolveSkipped()}
            className="group h-auto w-full justify-start gap-3 rounded-2xl border-zinc-200 bg-white px-4 py-3.5 text-left font-normal hover:bg-zinc-50"
          >
            <span className="p-2 rounded-xl bg-zinc-100 text-zinc-600 group-hover:bg-zinc-200">
              <Check size={16} strokeWidth={2.5} />
            </span>
            <span>
              <span className="block text-xs font-black uppercase tracking-wide text-darkDelegation">
                Skip media
              </span>
              <span className="block text-[11px] text-zinc-500 mt-0.5">
                Mark the project complete with your text summary only (no {label} file).
              </span>
            </span>
          </Button>

          <Button
            type="button"
            variant="outline"
            onClick={() => resolveMocked()}
            className="group h-auto w-full justify-start gap-3 rounded-2xl border-zinc-200 bg-white px-4 py-3.5 text-left font-normal hover:bg-zinc-50"
          >
            <span className="p-2 rounded-xl bg-violet-50 text-violet-600 group-hover:bg-violet-100">
              <TypeIcon t={outputType} />
            </span>
            <span>
              <span className="block text-xs font-black uppercase tracking-wide text-darkDelegation">
                Use mock placeholder
              </span>
              <span className="block text-[11px] text-zinc-500 mt-0.5">
                {outputType === 'image'
                  ? 'A tiny placeholder image for UI testing.'
                  : `A labeled placeholder instead of real ${label} (good for local QA).`}
              </span>
            </span>
          </Button>

          <Button
            type="button"
            variant="outline"
            onClick={() => resolveDeferred()}
            className="group h-auto w-full justify-start gap-3 rounded-2xl border-zinc-200 bg-white px-4 py-3.5 text-left font-normal hover:bg-zinc-50"
          >
            <span className="p-2 rounded-xl bg-sky-50 text-sky-600 group-hover:bg-sky-100">
              <Clock size={16} strokeWidth={2.5} />
            </span>
            <span>
              <span className="block text-xs font-black uppercase tracking-wide text-darkDelegation">
                Decide later
              </span>
              <span className="block text-[11px] text-zinc-500 mt-0.5">
                Stay here without auto-retry. Set <code className="text-[9px] font-mono">VITE_GEMINI_API_KEY</code> when
                needed for Gemini, or switch routing/backend in <code className="text-[9px] font-mono">model-config.ts</code>.
              </span>
            </span>
          </Button>
        </div>

        <p className="px-6 pb-5 text-[10px] text-zinc-400 leading-relaxed flex items-start gap-2">
          <Info size={14} className="shrink-0 mt-0.5 opacity-60" />
          By default, image / audio / video follow the same backend as agent chat (<code className="text-[9px] font-mono">follow-chat</code> in{' '}
          <code className="text-[9px] font-mono">model-config.ts</code>). Unsupported local media still fails gracefully without calling cloud APIs.
        </p>
      </div>
    </ModalRoot>
  );
}
