import React, { useState } from 'react';
import { X, Code, Copy, Check } from 'lucide-react';
import { Button } from '@/components/ui/button';
import type { AgenticSystem } from '../../data/agents';

interface SystemDebugOverlayProps {
  system: AgenticSystem;
}

export const SystemDebugOverlay: React.FC<SystemDebugOverlayProps> = ({ system }) => {
  const [isOpen, setIsOpen] = useState(false);
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    navigator.clipboard.writeText(JSON.stringify(system, null, 2));
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <>
      <Button
        type="button"
        variant="outline"
        onClick={() => setIsOpen(true)}
        className="flex h-auto items-center gap-1.5 rounded border-red-100 bg-red-50 px-2 py-1 text-[8px] font-black uppercase tracking-widest text-red-500 hover:bg-red-100"
      >
        <Code size={10} />
        Debug System
      </Button>

      {isOpen && (
        <div className="fixed inset-4 bg-white border border-zinc-200 shadow-2xl rounded-2xl z-[1000] flex flex-col overflow-hidden animate-in fade-in zoom-in-95 duration-200">
          <div className="p-4 border-b border-zinc-100 flex items-center justify-between bg-zinc-50">
            <div className="flex items-center gap-3">
              <Code size={14} className="text-zinc-400" />
              <h3 className="text-[10px] font-black uppercase tracking-widest text-zinc-500">
                System Debug Data — <span className="text-darkDelegation">{system.teamName || 'Untitled'}</span>
              </h3>
            </div>

            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="outline"
                onClick={handleCopy}
                className="flex h-auto items-center gap-1.5 rounded-lg border-zinc-200 bg-white px-2 py-1 text-[9px] font-bold text-zinc-600 hover:bg-zinc-50 active:scale-95"
              >
                {copied ? <Check size={12} className="text-green-500" /> : <Copy size={12} />}
                {copied ? 'Copied!' : 'Copy JSON'}
              </Button>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setIsOpen(false)}
                className="rounded-lg px-2 py-1 text-zinc-400 hover:bg-zinc-200 hover:text-darkDelegation"
                title="Close overlay"
              >
                <X size={16} />
              </Button>
            </div>
          </div>

          <pre className="flex-1 overflow-auto p-6 text-[11px] font-mono whitespace-pre-wrap bg-darkDelegation text-green-400 selection:bg-green-500/20">
            {JSON.stringify(system, null, 2)}
          </pre>

          <div className="p-3 bg-zinc-50 border-t border-zinc-100 flex justify-end">
            <p className="text-[8px] font-bold uppercase tracking-widest text-zinc-400 italic">
              Provisional Debug Tool • Close with ESC or button
            </p>
          </div>
        </div>
      )}
    </>
  );
};
