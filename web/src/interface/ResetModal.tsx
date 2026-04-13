import { AlertTriangle, FolderPlus, RefreshCcw, X } from 'lucide-react';
import React, { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { ModalBackdrop, ModalPanel, ModalRoot } from './components/ModalChrome';

interface ResetModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** When server-backed projects are off: single destructive reset (local only). */
  serverProjectsEnabled: boolean;
  /** Clears this project slot: board + local state; with server projects, empty payload is flushed to the server. */
  onConfirmClearThisProject: () => void | Promise<void>;
  /** Saves the current project, creates a new one on the server, and switches to it — previous projects stay in the list. */
  onConfirmStartFreshProject?: (userTitle: string) => Promise<void>;
}

const ResetModal: React.FC<ResetModalProps> = ({
  isOpen,
  onClose,
  serverProjectsEnabled,
  onConfirmClearThisProject,
  onConfirmStartFreshProject,
}) => {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [freshTitle, setFreshTitle] = useState('');

  useEffect(() => {
    if (isOpen) {
      setError(null);
      setBusy(false);
      setFreshTitle('');
    }
  }, [isOpen]);

  if (!isOpen) return null;

  const canStartFresh = serverProjectsEnabled && typeof onConfirmStartFreshProject === 'function';

  const runFresh = async () => {
    if (!onConfirmStartFreshProject) return;
    const t = freshTitle.trim();
    if (!t) {
      setError('Enter a name for the new project.');
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await onConfirmStartFreshProject(t);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const runClearOnly = async () => {
    if (busy) return;
    setError(null);
    setBusy(true);
    try {
      await Promise.resolve(onConfirmClearThisProject());
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ModalRoot>
      <ModalBackdrop onRequestClose={() => !busy && onClose()} />
      <ModalPanel>
        <div className="px-8 pt-8 pb-10">
          <div className="flex items-start justify-between mb-8">
            <div className="flex items-center gap-4">
              <div className="w-14 h-14 rounded-2xl bg-red-50 flex items-center justify-center text-red-500 shadow-sm shadow-red-100">
                <AlertTriangle size={32} strokeWidth={2.5} />
              </div>
              <h3 className="text-2xl font-black text-darkDelegation leading-tight">
                {canStartFresh ? 'New or reset project' : 'Start New Project?'}
              </h3>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              disabled={busy}
              onClick={onClose}
              className="rounded-full text-zinc-400 hover:bg-zinc-100 disabled:opacity-50"
              aria-label="Close"
            >
              <X size={20} />
            </Button>
          </div>

          {canStartFresh ? (
            <>
              <p className="text-sm text-zinc-500 leading-relaxed mb-4">
                Your work is saved on the server for each project. You can switch back anytime from the header
                <strong className="font-semibold text-zinc-600"> Project </strong>
                menu.
              </p>
              <ul className="text-sm text-zinc-600 space-y-2 mb-6 list-disc pl-5">
                <li>
                  <strong className="font-semibold text-zinc-700">Start a new project</strong> keeps this one in the
                  list and opens a blank board under a new saved project.
                </li>
                <li>
                  <strong className="font-semibold text-zinc-700">Clear this project only</strong> wipes the board for
                  the <em>current</em> project and replaces its saved data when it syncs — use only if you mean to
                  discard this slot.
                </li>
              </ul>
              <label className="block text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-2">
                New project name
              </label>
              <Input
                type="text"
                value={freshTitle}
                onChange={(e) => setFreshTitle(e.target.value)}
                disabled={busy}
                className="mb-6 h-auto rounded-2xl border-zinc-200 px-4 py-3 text-sm focus-visible:ring-darkDelegation/20 disabled:opacity-60"
                placeholder="Required"
                maxLength={200}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && freshTitle.trim()) void runFresh();
                }}
              />
            </>
          ) : (
            <p className="text-sm text-zinc-500 leading-relaxed mb-8">
              This will clear the current user brief, all tasks, logs, and conversation histories. The team will return
              to their starting positions and the project will revert to idle. In local-only mode, your work stays in this
              browser and is not synced to the server.
            </p>
          )}

          {error ? (
            <p className="text-sm text-red-600 mb-4 bg-red-50 border border-red-100 rounded-xl px-3 py-2">{error}</p>
          ) : null}

          <div className="flex flex-col gap-3">
            {canStartFresh ? (
              <>
                <Button
                  type="button"
                  disabled={busy || !freshTitle.trim()}
                  onClick={() => void runFresh()}
                  className="flex w-full items-center justify-center gap-2 rounded-2xl bg-darkDelegation py-4 font-black text-xs text-white uppercase tracking-widest hover:bg-black active:scale-[0.98] disabled:opacity-60"
                >
                  <FolderPlus size={14} />
                  {busy ? 'Creating…' : 'Start a new project (save this one)'}
                </Button>
                <Button
                  type="button"
                  variant="secondary"
                  disabled={busy}
                  onClick={runClearOnly}
                  className="flex w-full items-center justify-center gap-2 rounded-2xl py-4 font-black text-xs text-zinc-700 uppercase tracking-widest hover:bg-zinc-200 active:scale-[0.98]"
                >
                  <RefreshCcw size={14} />
                  Clear this project only
                </Button>
              </>
            ) : (
              <Button
                type="button"
                disabled={busy}
                onClick={runClearOnly}
                className="flex w-full items-center justify-center gap-2 rounded-2xl bg-darkDelegation py-4 font-black text-xs text-white uppercase tracking-widest hover:bg-black active:scale-[0.98]"
              >
                <RefreshCcw size={14} />
                Yes, Reset Everything
              </Button>
            )}
            <Button
              type="button"
              variant="secondary"
              disabled={busy}
              onClick={onClose}
              className="w-full rounded-2xl py-4 font-black text-xs text-zinc-600 uppercase tracking-widest hover:bg-zinc-200 active:scale-[0.98] disabled:opacity-60"
            >
              Cancel
            </Button>
          </div>
        </div>
      </ModalPanel>
    </ModalRoot>
  );
};

export default ResetModal;
