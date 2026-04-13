import { AlertTriangle, Trash2, X } from 'lucide-react';
import React, { useEffect, useId } from 'react';
import { Button } from '@/components/ui/button';
import { UI_LAYER_Z } from '../ui/uiLayers';
import { ModalBackdrop } from './ModalChrome';
import { cn } from '@/lib/utils';

export type ConfirmModalVariant = 'default' | 'danger';

export interface ConfirmModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** Called when the user activates the primary action. Close the modal from the parent when finished. */
  onConfirm: () => void;
  title: string;
  description: React.ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  variant?: ConfirmModalVariant;
  /** When omitted, danger uses a trash icon and default uses a warning icon */
  icon?: React.ReactNode | null;
  busy?: boolean;
  /** Override stacking; default is {@link UI_LAYER_Z.modal}. */
  zIndexClass?: string;
  size?: 'sm' | 'md';
  /** Row: cancel | confirm side by side (compact). Stack: primary then cancel (full width). */
  footerLayout?: 'stack' | 'row';
}

const ConfirmModal: React.FC<ConfirmModalProps> = ({
  isOpen,
  onClose,
  onConfirm,
  title,
  description,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  variant = 'default',
  icon,
  busy = false,
  zIndexClass = UI_LAYER_Z.modal,
  size = 'md',
  footerLayout = 'stack',
}) => {
  const titleId = useId();

  useEffect(() => {
    if (!isOpen || busy) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [isOpen, busy, onClose]);

  if (!isOpen) return null;

  const isSm = size === 'sm';
  const maxW = isSm ? 'max-w-sm' : 'max-w-md';
  const rounded = isSm ? 'rounded-2xl' : 'rounded-4xl';
  const pad = isSm ? 'p-5' : 'px-8 pt-8 pb-10';
  const titleClass = isSm
    ? 'text-lg font-bold text-darkDelegation mb-1.5 leading-tight'
    : 'text-2xl font-black text-darkDelegation leading-tight';
  const descClass = isSm ? 'text-[13px] text-zinc-500 leading-relaxed mb-6' : 'text-sm text-zinc-500 leading-relaxed mb-8';
  const iconWrap = isSm ? 'w-10 h-10 rounded-xl' : 'w-14 h-14 rounded-2xl';

  const defaultIcon =
    variant === 'danger' ? (
      <div className={`${iconWrap} bg-red-50 flex items-center justify-center text-red-500`}>
        <Trash2 size={isSm ? 20 : 24} strokeWidth={2.5} />
      </div>
    ) : (
      <div className={`${iconWrap} bg-amber-50 flex items-center justify-center text-amber-600 shadow-sm`}>
        <AlertTriangle size={isSm ? 22 : 32} strokeWidth={2.5} />
      </div>
    );

  /** null = no icon; undefined = built-in icon for variant */
  const resolvedIcon = icon === null ? null : icon === undefined ? defaultIcon : icon;

  const primaryBtnClass = cn(
    'rounded-2xl font-bold transition-all active:scale-[0.98] disabled:opacity-60',
    variant === 'danger'
      ? 'bg-red-500 text-white shadow-sm shadow-red-200 hover:bg-red-600'
      : 'bg-darkDelegation text-white hover:bg-black',
    footerLayout === 'row' ? 'flex-1 py-2 text-sm' : 'w-full py-4 font-black text-xs uppercase tracking-widest',
  );

  const secondaryBtnClass = cn(
    'rounded-2xl font-bold transition-colors disabled:opacity-50',
    footerLayout === 'row'
      ? 'flex-1 py-2 text-sm text-zinc-500 hover:bg-zinc-50'
      : 'w-full bg-zinc-100 py-4 font-black text-xs text-zinc-600 uppercase tracking-widest hover:bg-zinc-200 active:scale-[0.98] disabled:opacity-60',
  );

  const handleBackdrop = () => {
    if (!busy) onClose();
  };

  return (
    <div
      className={`fixed inset-0 ${zIndexClass} flex items-center justify-center p-4`}
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
    >
      <ModalBackdrop onRequestClose={handleBackdrop} />
      <div
        className={`relative w-full ${maxW} bg-white ${rounded} shadow-2xl overflow-hidden border border-zinc-100`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className={pad}>
          <div className={`flex items-start justify-between ${isSm ? 'mb-4' : 'mb-8'}`}>
            <div className="flex items-center gap-4 min-w-0">
              {resolvedIcon ? <div className="shrink-0">{resolvedIcon}</div> : null}
              <h3 id={titleId} className={`${titleClass} min-w-0`}>
                {title}
              </h3>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              disabled={busy}
              onClick={onClose}
              className="shrink-0 rounded-full text-zinc-400 hover:bg-zinc-100 hover:text-zinc-600"
              aria-label="Close"
            >
              <X size={isSm ? 18 : 20} />
            </Button>
          </div>

          <div className={descClass}>{description}</div>

          {footerLayout === 'row' ? (
            <div className="flex items-center gap-2">
              <Button type="button" variant="ghost" disabled={busy} onClick={onClose} className={secondaryBtnClass}>
                {cancelLabel}
              </Button>
              <Button
                type="button"
                variant="ghost"
                disabled={busy}
                onClick={() => {
                  if (busy) return;
                  onConfirm();
                }}
                className={primaryBtnClass}
              >
                {confirmLabel}
              </Button>
            </div>
          ) : (
            <div className="flex flex-col gap-3">
              <Button
                type="button"
                variant="ghost"
                disabled={busy}
                onClick={() => {
                  if (busy) return;
                  onConfirm();
                }}
                className={primaryBtnClass}
              >
                {confirmLabel}
              </Button>
              <Button type="button" variant="ghost" disabled={busy} onClick={onClose} className={secondaryBtnClass}>
                {cancelLabel}
              </Button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};

export default ConfirmModal;
