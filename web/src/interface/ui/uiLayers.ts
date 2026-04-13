/**
 * Canonical z-index classes for fixed overlays (Tailwind). Use these instead of ad-hoc z-* so stacking stays predictable.
 */
export const UI_LAYER_Z = {
  /** Below standard modals (e.g. full-width flow preview). */
  drawer: 'z-50',
  /** Default dialogs: reset, BYOK, confirm, pricing. */
  modal: 'z-[100]',
  /** Above default modal, below nested (e.g. multimodal blocked notice). */
  modalElevated: 'z-[110]',
  /** Nested above default modal (e.g. project name over app shell). */
  modalNested: 'z-[120]',
  /** Chat / nested flows that must sit above review UI. */
  modalAlert: 'z-[140]',
  /** Task audit / rare full-screen attention. */
  audit: 'z-[9999]',
} as const;

export type UiLayerKey = keyof typeof UI_LAYER_Z;

/** Backdrop visual presets paired with {@link UI_BACKDROP_CLASS}. */
export type UiBackdropTone = 'lightSoft' | 'lightXl' | 'darkDelegation' | 'zinc' | 'dim';

export const UI_BACKDROP_CLASS: Record<UiBackdropTone, string> = {
  lightSoft: 'bg-white/60 backdrop-blur-sm',
  lightXl: 'bg-white/60 backdrop-blur-xl',
  darkDelegation: 'bg-darkDelegation/40 backdrop-blur-md',
  zinc: 'bg-zinc-900/40 backdrop-blur-sm',
  dim: 'bg-black/40 backdrop-blur-sm',
};
