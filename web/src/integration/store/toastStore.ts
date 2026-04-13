import { create } from 'zustand';

export type ToastVariant = 'info' | 'success' | 'error';

export type ToastItem = {
  id: string;
  message: string;
  variant: ToastVariant;
};

type ToastState = {
  items: ToastItem[];
  push: (message: string, variant?: ToastVariant) => string;
  dismiss: (id: string) => void;
};

let idSeq = 0;

function defaultDuration(variant: ToastVariant): number {
  return variant === 'error' ? 6500 : 4200;
}

export const useToastStore = create<ToastState>((set) => ({
  items: [],
  push(message, variant = 'info') {
    const id = `toast-${++idSeq}`;
    set((s) => ({ items: [...s.items, { id, message, variant }] }));
    window.setTimeout(() => {
      set((s) => ({ items: s.items.filter((t) => t.id !== id) }));
    }, defaultDuration(variant));
    return id;
  },
  dismiss(id) {
    set((s) => ({ items: s.items.filter((t) => t.id !== id) }));
  },
}));

/** Fire-and-forget toast (no hook required). */
export function showToast(message: string, variant: ToastVariant = 'info'): string {
  return useToastStore.getState().push(message, variant);
}
