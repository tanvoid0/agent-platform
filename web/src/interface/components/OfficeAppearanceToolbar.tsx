import React from 'react';
import { Button } from '@/components/ui/button';
import { useCoreStore } from '../../integration/store/coreStore';
import { commitOfficeVisualStyle } from './officeAppearance';
import type { OfficeVisualStyle } from '../../types';

export const OFFICE_STYLE_OPTIONS: { id: OfficeVisualStyle; label: string; title: string }[] = [
  { id: 'color', label: 'Color', title: 'Tinted furniture, plants, and team accent on trim' },
  { id: 'monochrome', label: 'Mono', title: 'Grayscale office while keeping contrast' },
  { id: 'performant', label: 'Fast', title: 'Hide plants, simpler shadows, cap pixel ratio for weaker GPUs' },
];

/**
 * Color / Mono / Fast — updates `officeVisualStyle` (persisted in local store).
 */
export const OfficeAppearanceToolbar: React.FC<{ className?: string }> = ({ className }) => {
  const officeVisualStyle = useCoreStore((s) => s.officeVisualStyle);

  return (
    <div className={className}>
      <select
        className="md:hidden text-[11px] border border-zinc-200/80 rounded-lg bg-zinc-50/90 px-2 py-1.5 text-zinc-700 max-w-[6.5rem] shrink-0 cursor-pointer"
        aria-label="Office appearance"
        value={officeVisualStyle}
        onChange={(e) => commitOfficeVisualStyle(e.target.value as OfficeVisualStyle)}
      >
        {OFFICE_STYLE_OPTIONS.map((opt) => (
          <option key={opt.id} value={opt.id}>
            {opt.label}
          </option>
        ))}
      </select>

      <div
        className="hidden md:flex items-center rounded-full border border-zinc-200/80 bg-zinc-50/90 p-0.5 text-[11px] shrink-0"
        role="group"
        aria-label="Office appearance"
      >
        {OFFICE_STYLE_OPTIONS.map((opt) => (
          <Button
            key={opt.id}
            type="button"
            variant="ghost"
            title={opt.title}
            onClick={() => commitOfficeVisualStyle(opt.id)}
            className={`h-auto rounded-full px-2.5 py-1 font-normal ${
              officeVisualStyle === opt.id
                ? 'border border-zinc-200/60 bg-white text-darkDelegation shadow-sm hover:bg-white'
                : 'text-zinc-500 hover:text-zinc-800'
            }`}
          >
            {opt.label}
          </Button>
        ))}
      </div>
    </div>
  );
};
