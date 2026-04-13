import React from 'react';
import type { ConnectivityLineReadout } from '../../integration/hooks/useConnectivityReadout';

function toneClass(t: ConnectivityLineReadout['tone']): string {
  switch (t) {
    case 'ok':
      return 'text-emerald-700';
    case 'warn':
      return 'text-amber-700';
    case 'bad':
      return 'text-red-600';
    default:
      return 'text-zinc-500';
  }
}

export const HeaderConnectivityReadout: React.FC<{
  projects: ConnectivityLineReadout;
  llm: ConnectivityLineReadout;
}> = ({ projects, llm }) => (
  <div
    className="hidden min-w-0 max-w-[min(340px,30vw)] flex-col items-end justify-center gap-0.5 pr-1 lg:flex"
    aria-live="polite"
  >
    <div className="w-full text-right" title={projects.detail}>
      <span className="text-[8px] font-black uppercase tracking-wider text-zinc-400">API</span>
      <span
        className={`ml-1.5 text-[9px] font-mono font-bold tabular-nums leading-snug ${toneClass(projects.tone)}`}
      >
        {projects.short}
      </span>
    </div>
    <div className="w-full text-right" title={llm.detail}>
      <span className="text-[8px] font-black uppercase tracking-wider text-zinc-400">LLM</span>
      <span
        className={`ml-1.5 text-[9px] font-mono font-bold tabular-nums leading-snug ${toneClass(llm.tone)}`}
      >
        {llm.short}
      </span>
    </div>
  </div>
);
