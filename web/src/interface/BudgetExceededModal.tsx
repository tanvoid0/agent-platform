import { Wallet, X } from 'lucide-react';
import React from 'react';
import { Link } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import { useCoreStore } from '../integration/store/coreStore';
import { ModalBackdrop, ModalPanel, ModalRoot } from './components/ModalChrome';

export const BudgetExceededModal: React.FC = () => {
  const budgetExceededOpen = useCoreStore((s) => s.budgetExceededOpen);
  const budgetExceededMessage = useCoreStore((s) => s.budgetExceededMessage);
  const closeBudgetExceeded = useCoreStore((s) => s.closeBudgetExceeded);

  if (!budgetExceededOpen) return null;

  return (
    <ModalRoot layer="modalNested">
      <ModalBackdrop
        tone="zinc"
        onRequestClose={() => closeBudgetExceeded()}
        className="cursor-default"
        aria-label="Close"
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') closeBudgetExceeded();
        }}
      />
      <ModalPanel className="rounded-2xl border-zinc-200">
        <div className="px-6 pt-6 pb-6">
          <div className="flex items-start justify-between gap-4 mb-4">
            <div className="flex items-center gap-3 min-w-0">
              <div className="w-12 h-12 rounded-xl bg-amber-50 flex items-center justify-center text-amber-600 shrink-0">
                <Wallet size={26} strokeWidth={2.2} />
              </div>
              <h3 className="text-lg font-black text-darkDelegation leading-tight">Budget limit reached</h3>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              onClick={() => closeBudgetExceeded()}
              className="shrink-0 text-zinc-400 hover:text-zinc-600"
              aria-label="Close"
            >
              <X size={20} />
            </Button>
          </div>
          <p className="text-sm text-zinc-600 leading-relaxed mb-6">{budgetExceededMessage}</p>
          <div className="flex flex-col gap-2 sm:flex-row sm:justify-end">
            <Button
              type="button"
              variant="outline"
              onClick={() => closeBudgetExceeded()}
              className="rounded-xl border-zinc-200 px-4 py-2.5 text-sm font-bold text-zinc-700 hover:bg-zinc-50"
            >
              Dismiss
            </Button>
            <Button variant="default" className="rounded-xl bg-darkDelegation px-4 py-2.5 text-center text-sm font-black uppercase tracking-wide text-white hover:bg-black" asChild>
              <Link to="/finance/project" onClick={() => closeBudgetExceeded()}>
                Open Finance
              </Link>
            </Button>
          </div>
        </div>
      </ModalPanel>
    </ModalRoot>
  );
};
