import React from 'react';
import type { UiBackdropTone, UiLayerKey } from '../ui/uiLayers';
import { UI_BACKDROP_CLASS, UI_LAYER_Z } from '../ui/uiLayers';

export type ModalRootProps = React.HTMLAttributes<HTMLDivElement> & {
  layer?: UiLayerKey;
  /** Outer padding (default matches most modals). */
  paddingClassName?: string;
};

/**
 * Full-viewport flex centering shell. Compose with {@link ModalBackdrop} and your panel content.
 */
export function ModalRoot({
  layer = 'modal',
  paddingClassName = 'p-4',
  className = '',
  children,
  ...rest
}: ModalRootProps) {
  return (
    <div
      className={`fixed inset-0 ${UI_LAYER_Z[layer]} flex items-center justify-center ${paddingClassName} ${className}`.trim()}
      {...rest}
    >
      {children}
    </div>
  );
}

export type ModalBackdropProps = Omit<React.HTMLAttributes<HTMLDivElement>, 'children'> & {
  tone?: UiBackdropTone;
  onRequestClose?: () => void;
};

export function ModalBackdrop({
  tone = 'lightSoft',
  onRequestClose,
  className = '',
  role = 'presentation',
  ...rest
}: ModalBackdropProps) {
  return (
    <div
      role={role}
      className={`absolute inset-0 ${UI_BACKDROP_CLASS[tone]} ${className}`.trim()}
      onClick={onRequestClose}
      {...rest}
    />
  );
}

export type ModalPanelProps = React.HTMLAttributes<HTMLDivElement> & {
  maxWidthClass?: string;
};

/** Default white card used by most dialogs; override `className` for rounded-[40px] variants. */
export function ModalPanel({
  maxWidthClass = 'max-w-md',
  className = '',
  onMouseDown,
  children,
  ...rest
}: ModalPanelProps) {
  return (
    <div
      className={`relative w-full ${maxWidthClass} bg-white rounded-4xl shadow-2xl overflow-hidden border border-zinc-100 ${className}`.trim()}
      onMouseDown={(e) => {
        e.stopPropagation();
        onMouseDown?.(e);
      }}
      {...rest}
    >
      {children}
    </div>
  );
}
