import { X } from 'lucide-react';
import React, { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  cancelProjectTitlePrompt,
  confirmDiscussWithConsultantFirst,
  confirmNewProjectWithName,
  confirmProjectTitle,
  useProjectTitlePromptStore,
} from '../integration/projectTitlePrompt';
import { ModalBackdrop, ModalPanel, ModalRoot } from './components/ModalChrome';

const NewProjectNameModal: React.FC = () => {
  const open = useProjectTitlePromptStore((s) => s.open);
  const headline = useProjectTitlePromptStore((s) => s.headline);
  const mode = useProjectTitlePromptStore((s) => s.mode);
  const [value, setValue] = useState('');

  useEffect(() => {
    if (open) setValue('');
  }, [open]);

  if (!open) return null;

  const trimmed = value.trim();
  const canSubmit = trimmed.length > 0;

  return (
    <ModalRoot layer="modalNested">
      <ModalBackdrop onRequestClose={() => cancelProjectTitlePrompt()} aria-hidden />
      <ModalPanel>
        <div className="px-8 pt-8 pb-8">
          <div className="flex items-start justify-between mb-6">
            <h3 className="text-xl font-black text-darkDelegation leading-tight pr-4">{headline}</h3>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              onClick={() => cancelProjectTitlePrompt()}
              className="shrink-0 rounded-full text-zinc-400 hover:bg-zinc-100"
              aria-label="Close"
            >
              <X size={20} />
            </Button>
          </div>
          {mode === 'simple' && (
            <>
              <label className="block text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-2">
                Project name
              </label>
              <Input
                type="text"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                className="mb-6 h-auto rounded-2xl border-zinc-200 px-4 py-3 text-sm focus-visible:ring-darkDelegation/20"
                placeholder="e.g. Q1 landing page"
                maxLength={200}
                autoFocus
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && canSubmit) confirmProjectTitle(value);
                  if (e.key === 'Escape') cancelProjectTitlePrompt();
                }}
              />
            </>
          )}
          <div className="flex flex-col gap-2">
            {mode === 'newProject' ? (
              <>
                <Button
                  type="button"
                  autoFocus
                  onClick={() => confirmDiscussWithConsultantFirst()}
                  className="w-full rounded-2xl bg-teal-700 py-4 font-black text-xs text-white uppercase tracking-widest hover:bg-teal-900 active:scale-[0.98]"
                >
                  Discuss with consultant
                </Button>
                <p className="text-center text-[11px] text-zinc-500 leading-snug -mt-1">
                  No name needed—the Consultant can help you choose one. Opens chat on a fresh project.
                </p>
                <div className="border-t border-zinc-100 pt-4 mt-2 space-y-3">
                  <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">
                    Or name it yourself
                  </p>
                  <label className="block text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-2">
                    Project name
                  </label>
                  <Input
                    type="text"
                    value={value}
                    onChange={(e) => setValue(e.target.value)}
                    className="h-auto rounded-2xl border-zinc-200 px-4 py-3 text-sm focus-visible:ring-darkDelegation/20"
                    placeholder="e.g. Q1 landing page"
                    maxLength={200}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && canSubmit) confirmNewProjectWithName(value);
                      if (e.key === 'Escape') cancelProjectTitlePrompt();
                    }}
                  />
                  <Button
                    type="button"
                    disabled={!canSubmit}
                    onClick={() => confirmNewProjectWithName(value)}
                    className="w-full rounded-2xl bg-darkDelegation py-4 font-black text-xs text-white uppercase tracking-widest hover:bg-black active:scale-[0.98] disabled:opacity-50"
                  >
                    Create project with this name
                  </Button>
                </div>
              </>
            ) : (
              <Button
                type="button"
                disabled={!canSubmit}
                onClick={() => confirmProjectTitle(value)}
                className="w-full rounded-2xl bg-darkDelegation py-4 font-black text-xs text-white uppercase tracking-widest hover:bg-black active:scale-[0.98] disabled:opacity-50"
              >
                Create project
              </Button>
            )}
            <Button
              type="button"
              variant="secondary"
              onClick={() => cancelProjectTitlePrompt()}
              className="w-full rounded-2xl py-4 font-black text-xs text-zinc-600 uppercase tracking-widest hover:bg-zinc-200 active:scale-[0.98]"
            >
              Cancel
            </Button>
          </div>
        </div>
      </ModalPanel>
    </ModalRoot>
  );
};

export default NewProjectNameModal;
