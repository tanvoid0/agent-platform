import { ExternalLink, X, Sparkles } from 'lucide-react';
import React from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent } from '@/components/ui/dialog';
import { ScrollArea } from '@/components/ui/scroll-area';
import { GEMINI_PRICING } from '../core/llm/pricing';
import { DEFAULT_MODELS } from '../core/llm/constants';

type VideoMusicKey = 'video' | 'music';

interface PricingModalProps {
  onClose: () => void;
}

const PricingModal: React.FC<PricingModalProps> = ({ onClose }) => {
  const reasoningModels = Object.entries(GEMINI_PRICING)
    .filter(([_, p]) => p.inputPer1M !== undefined)
    .sort(([a], [b]) => (a === DEFAULT_MODELS.text ? -1 : (b === DEFAULT_MODELS.text ? 1 : 0)));
  const outputModels = Object.entries(GEMINI_PRICING).filter(([_, p]) => p.inputPer1M === undefined);

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        showCloseButton={false}
        overlayClassName="bg-white/60 backdrop-blur-xl"
        className="max-h-[90vh] max-w-4xl gap-0 overflow-hidden rounded-[40px] border-zinc-100 bg-white p-0 text-foreground shadow-[0_32px_64px_-12px_rgba(0,0,0,0.1)] ring-black/5 sm:max-w-4xl"
      >
        <ScrollArea className="max-h-[90vh]">
          <div className="relative p-8 md:p-12">
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="absolute top-6 right-6 text-muted-foreground hover:text-foreground"
              onClick={onClose}
              aria-label="Close"
            >
              <X size={20} />
            </Button>

            <div className="mx-auto">
              <div className="mb-10 text-center">
                <h2 className="mb-2 text-3xl font-black tracking-tight text-darkDelegation">
                  Gemini API Pricing
                </h2>
                <div className="flex flex-col items-center gap-3">
                  <Button variant="outline" size="sm" className="rounded-full border-blue-100 bg-blue-50 text-blue-600 hover:bg-blue-100 hover:border-blue-200" asChild>
                    <a
                      href="https://ai.google.dev/gemini-api/docs/pricing"
                      target="_blank"
                      rel="noopener noreferrer"
                      className="inline-flex items-center gap-2"
                    >
                      <span className="text-[11px] font-black uppercase tracking-wider">Official Pricing Page</span>
                      <ExternalLink size={11} className="text-blue-500" />
                    </a>
                  </Button>
                  <p className="text-xs font-medium leading-relaxed text-zinc-500">
                    Official Google Gemini API pricing (March 2026).
                  </p>
                </div>
              </div>

              <div className="grid grid-cols-1 gap-12 md:grid-cols-2">
                <div className="space-y-10">
                  <div className="space-y-6">
                    <h3 className="border-l-2 border-blue-500 px-1 pl-3 text-[11px] font-black uppercase tracking-[0.2em] text-zinc-500">
                      Reasoning Models
                    </h3>
                    <div className="space-y-3">
                      {reasoningModels.map(([model, pricing]) => {
                        const isDefault = model === DEFAULT_MODELS.text;
                        return (
                          <div
                            key={model}
                            className={`relative flex items-center justify-between rounded-2xl border px-5 py-3.5 transition-all duration-300 ${
                              isDefault ? 'border-blue-100 bg-blue-50/80 shadow-sm' : 'border-zinc-100/60 bg-zinc-50'
                            }`}
                          >
                            {isDefault && (
                              <div className="absolute -top-2 left-4 flex items-center gap-1.5 rounded-full bg-blue-600 px-2 py-0.5 text-[8px] font-black tracking-widest text-white uppercase shadow-sm">
                                <Sparkles size={8} className="fill-white" />
                                Default
                              </div>
                            )}
                            <div className="flex min-w-0 flex-1 items-center gap-3">
                              <p className="text-xs font-bold text-darkDelegation lowercase">{model}</p>
                              {isDefault && <Sparkles size={10} className="text-blue-500" />}
                            </div>
                            <div className="flex items-center gap-5 font-mono text-xs font-bold">
                              <div className="flex items-center gap-2">
                                <span className="text-[10px] font-medium tracking-tighter text-zinc-400 uppercase">In</span>
                                <span className="text-darkDelegation">${pricing.inputPer1M?.toFixed(2)}</span>
                              </div>
                              <div className="flex items-center gap-2">
                                <span className="text-[10px] font-medium tracking-tighter text-zinc-400 uppercase">Out</span>
                                <span className="text-darkDelegation">${pricing.outputPer1M?.toFixed(2)}</span>
                              </div>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  </div>

                  {['image'].map((type) => {
                    const typeModels = outputModels
                      .filter(([_, p]) => p.perImage !== undefined)
                      .sort(([a], [b]) => {
                        const defaultModel = DEFAULT_MODELS.image;
                        return a === defaultModel ? -1 : b === defaultModel ? 1 : 0;
                      });

                    if (typeModels.length === 0) return null;

                    return (
                      <div key={type} className="space-y-6">
                        <h4 className="border-l-2 border-amber-400 px-1 pl-3 text-[11px] font-black uppercase tracking-[0.2em] text-zinc-500">
                          {type} models
                        </h4>
                        <div className="space-y-3">
                          {typeModels.map(([model, pricing]) => {
                            const isDefault = model === DEFAULT_MODELS.image;
                            return (
                              <div
                                key={model}
                                className={`relative flex items-center justify-between rounded-2xl border px-5 py-3.5 transition-all duration-300 ${
                                  isDefault ? 'border-amber-100 bg-amber-50/80 shadow-sm' : 'border-zinc-100/60 bg-zinc-50'
                                }`}
                              >
                                {isDefault && (
                                  <div className="absolute -top-2 left-4 flex items-center gap-1.5 rounded-full bg-amber-500 px-2 py-0.5 text-[8px] font-black tracking-widest text-white uppercase shadow-sm">
                                    <Sparkles size={8} className="fill-white" />
                                    Default
                                  </div>
                                )}
                                <div className="flex min-w-0 flex-1 items-center gap-3">
                                  <p className="text-xs font-bold text-darkDelegation lowercase">{model}</p>
                                  {isDefault && <Sparkles size={10} className="text-amber-500" />}
                                </div>
                                <div className="flex items-center gap-4">
                                  <span className="text-[10px] font-medium tracking-tight text-zinc-400 uppercase">Img</span>
                                  <span className="font-mono text-sm font-bold text-darkDelegation">${pricing.perImage?.toFixed(3)}</span>
                                </div>
                              </div>
                            );
                          })}
                        </div>
                      </div>
                    );
                  })}
                </div>

                <div className="space-y-10">
                  {(['video', 'music'] as const satisfies readonly VideoMusicKey[]).map((type) => {
                    const typeModels = outputModels
                      .filter(([_, p]) => {
                        if (type === 'music') return p.perSong !== undefined;
                        return p.perSecond !== undefined;
                      })
                      .sort(([a], [b]) => {
                        const defaultModel = DEFAULT_MODELS[type];
                        return a === defaultModel ? -1 : b === defaultModel ? 1 : 0;
                      });

                    if (typeModels.length === 0) return null;

                    const colors =
                      type === 'video'
                        ? {
                            border: 'border-rose-500',
                            bg: 'bg-rose-50/80',
                            badge: 'bg-rose-600',
                            borderLight: 'border-rose-100',
                            icon: 'text-rose-500',
                          }
                        : {
                            border: 'border-lime-400',
                            bg: 'bg-lime-50/80',
                            badge: 'bg-lime-500',
                            borderLight: 'border-lime-100',
                            icon: 'text-lime-600',
                          };

                    return (
                      <div key={type} className="space-y-6">
                        <h4
                          className={`border-l-2 ${colors.border} px-1 pl-3 text-[11px] font-black uppercase tracking-[0.2em] text-zinc-500`}
                        >
                          {type} models
                        </h4>
                        <div className="space-y-3">
                          {typeModels.map(([model, pricing]) => {
                            const isDefault = model === DEFAULT_MODELS[type];
                            const label =
                              pricing.perSong !== undefined
                                ? model === DEFAULT_MODELS.music
                                  ? '30 Sec Song'
                                  : 'Song'
                                : 'Sec';

                            return (
                              <div
                                key={model}
                                className={`relative flex items-center justify-between rounded-2xl border px-5 py-3.5 transition-all duration-300 ${
                                  isDefault ? `${colors.bg} ${colors.borderLight} shadow-sm` : 'border-zinc-100/60 bg-zinc-50'
                                }`}
                              >
                                {isDefault && (
                                  <div
                                    className={`absolute -top-2 left-4 flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[8px] font-black tracking-widest text-white uppercase shadow-sm ${colors.badge}`}
                                  >
                                    <Sparkles size={8} className="fill-white" />
                                    Default
                                  </div>
                                )}
                                <div className="flex min-w-0 flex-1 items-center gap-3">
                                  <p className="text-xs font-bold text-darkDelegation lowercase">{model}</p>
                                  {isDefault && <Sparkles size={10} className={colors.icon} />}
                                </div>
                                <div className="flex items-center gap-4">
                                  <span className="text-[10px] font-medium tracking-tight text-zinc-400 uppercase">{label}</span>
                                  <span className="font-mono text-sm font-bold text-darkDelegation">
                                    ${(pricing.perSong || pricing.perSecond || 0).toFixed(3)}
                                  </span>
                                </div>
                              </div>
                            );
                          })}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            </div>
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
};

export default PricingModal;
