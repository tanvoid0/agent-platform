import { CheckCircle2, Info, X, XCircle } from 'lucide-react';
import React from 'react';
import { Button } from '@/components/ui/button';
import { useToastStore, type ToastVariant } from '../../integration/store/toastStore';

function variantStyles(v: ToastVariant): { bar: string; icon: React.ReactNode } {
  switch (v) {
    case 'success':
      return {
        bar: 'border-emerald-200/80 bg-white text-zinc-800 shadow-emerald-100/50',
        icon: <CheckCircle2 className="text-emerald-600 shrink-0" size={18} strokeWidth={2.5} />,
      };
    case 'error':
      return {
        bar: 'border-red-200/80 bg-white text-zinc-800 shadow-red-100/50',
        icon: <XCircle className="text-red-600 shrink-0" size={18} strokeWidth={2.5} />,
      };
    default:
      return {
        bar: 'border-zinc-200/80 bg-white text-zinc-800 shadow-zinc-200/40',
        icon: <Info className="text-zinc-500 shrink-0" size={18} strokeWidth={2.5} />,
      };
  }
}

export const ToastViewport: React.FC = () => {
  const items = useToastStore((s) => s.items);
  const dismiss = useToastStore((s) => s.dismiss);

  if (items.length === 0) return null;

  return (
    <div
      className="fixed bottom-4 right-4 z-[200] flex flex-col gap-2 max-w-[min(100vw-2rem,22rem)] pointer-events-none"
      aria-live="polite"
    >
      {items.map((t) => {
        const { bar, icon } = variantStyles(t.variant);
        return (
          <div
            key={t.id}
            role="status"
            className={`pointer-events-auto flex items-start gap-3 pl-3 pr-2 py-2.5 rounded-2xl border shadow-lg backdrop-blur-sm ${bar}`}
          >
            {icon}
            <p className="text-[13px] font-medium leading-snug flex-1 pt-0.5">{t.message}</p>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              onClick={() => dismiss(t.id)}
              className="shrink-0 text-zinc-400 hover:bg-zinc-100 hover:text-zinc-600"
              aria-label="Dismiss"
            >
              <X size={16} strokeWidth={2.5} />
            </Button>
          </div>
        );
      })}
    </div>
  );
};
