import { ArrowLeft } from 'lucide-react';
import React from 'react';
import { Link } from 'react-router-dom';
import { VisualConfigurator } from './VisualConfigurator/VisualConfigurator';

export const TeamManagementPage: React.FC = () => {
  return (
    <div className="h-screen min-h-0 bg-zinc-50 flex flex-col overflow-hidden">
      <header className="h-14 border-b border-zinc-200 bg-white flex items-center justify-between px-4 sm:px-6 shrink-0 z-10">
        <div className="flex flex-col gap-0.5 min-w-0">
          <div className="flex items-center gap-3 min-w-0">
            <Link
              to="/"
              className="flex items-center gap-2 text-zinc-500 hover:text-darkDelegation text-xs font-bold uppercase tracking-wide shrink-0"
            >
              <ArrowLeft size={16} />
              Simulation
            </Link>
            <span className="text-zinc-200">|</span>
            <h1 className="text-sm font-black text-darkDelegation uppercase tracking-widest truncate">Teams</h1>
          </div>
          <p className="text-[10px] text-zinc-500 pl-0 sm:pl-[7.25rem] max-w-3xl leading-snug hidden sm:block">
            Use <span className="font-bold text-zinc-700">Consultant Workshop</span> in the list, then return to Simulation to plan execution teams and handoff briefs.
          </p>
        </div>
      </header>

      <main className="flex-1 min-h-0 flex flex-col overflow-hidden">
        <VisualConfigurator />
      </main>
    </div>
  );
};
