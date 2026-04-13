import React from 'react';
import { Button } from '@/components/ui/button';
import { commitOfficeVisualStyle } from '../components/officeAppearance';
import { useCoreStore } from '../../integration/store/coreStore';
import type { OfficeVisualStyle } from '../../types';

const OPTIONS: { id: OfficeVisualStyle; label: string; description: string }[] = [
  {
    id: 'color',
    label: 'Color',
    description: 'Tinted furniture, plants, and team accent on trim.',
  },
  {
    id: 'monochrome',
    label: 'Monochrome',
    description: 'Grayscale office while keeping readable contrast.',
  },
  {
    id: 'performant',
    label: 'Performant',
    description: 'Hide plants, simpler shadows, cap pixel ratio for weaker GPUs.',
  },
];

export const SceneAppearanceSection: React.FC = () => {
  const officeVisualStyle = useCoreStore((s) => s.officeVisualStyle);

  return (
    <div>
      <h2 className="text-xl font-black text-darkDelegation tracking-tight mb-1">3D office</h2>
      <p className="text-sm text-zinc-500 font-medium mb-5 max-w-xl">
        Balance visual richness and GPU cost. The same preset is available from the simulation toolbar on larger
        screens.
      </p>
      <div className="grid gap-3 sm:grid-cols-3">
        {OPTIONS.map((opt) => (
          <Button
            key={opt.id}
            type="button"
            variant="ghost"
            onClick={() => commitOfficeVisualStyle(opt.id)}
            className={`h-auto w-full justify-start whitespace-normal rounded-2xl border px-4 py-3 text-left font-normal transition-all ${
              officeVisualStyle === opt.id
                ? 'border-darkDelegation bg-zinc-900 text-white shadow-md hover:bg-zinc-900 hover:text-white'
                : 'border-zinc-200 bg-zinc-50/80 text-darkDelegation hover:border-zinc-300 hover:bg-zinc-50/80'
            }`}
          >
            <span className="block text-xs font-black uppercase tracking-wider">{opt.label}</span>
            <span
              className={`mt-1 block text-[10px] font-medium leading-snug ${
                officeVisualStyle === opt.id ? 'text-zinc-300' : 'text-zinc-500'
              }`}
            >
              {opt.description}
            </span>
          </Button>
        ))}
      </div>
    </div>
  );
};
