import { ArrowLeft, Loader2 } from 'lucide-react';
import React, { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { listRemoteProjects, type ProjectListEntry } from '../integration/api/projectRemoteApi';
import { formatTokens } from './formatTokens';

export const FinancePortfolioPage: React.FC = () => {
  const [projects, setProjects] = useState<ProjectListEntry[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    listRemoteProjects(100, 0)
      .then((r) => {
        if (!cancelled) setProjects(r.projects);
      })
      .catch((e) => {
        if (!cancelled) setLoadError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const rows = projects;

  const totals = useMemo(() => {
    let cost = 0;
    let tokens = 0;
    for (const p of rows) {
      const f = p.meta.finance;
      cost += f?.estimatedCostUsd ?? 0;
      tokens += f?.totalTokens ?? 0;
    }
    return { cost, tokens };
  }, [rows]);

  return (
    <div className="min-h-screen bg-zinc-50 flex flex-col">
      <header className="h-14 border-b border-zinc-200 bg-white flex items-center justify-between px-4 sm:px-6 shrink-0">
        <div className="flex items-center gap-3 min-w-0">
          <Link
            to="/"
            className="flex items-center gap-2 text-zinc-500 hover:text-darkDelegation text-xs font-bold uppercase tracking-wide shrink-0"
          >
            <ArrowLeft size={16} />
            Simulation
          </Link>
          <span className="text-zinc-200">|</span>
          <h1 className="text-sm font-black text-darkDelegation uppercase tracking-widest truncate">Finance</h1>
        </div>
        <div className="flex items-center gap-3 shrink-0">
          <Link
            to="/projects"
            className="text-xs font-bold text-zinc-500 hover:text-darkDelegation uppercase tracking-wide"
          >
            Projects
          </Link>
          <Link
            to="/finance/project"
            className="text-xs font-bold text-emerald-700 hover:text-emerald-800 uppercase tracking-tight"
          >
            Current project detail
          </Link>
        </div>
      </header>

      <main className="flex-1 max-w-5xl w-full mx-auto px-4 py-8">
        <p className="text-sm text-zinc-600 mb-6 bg-white border border-zinc-200 rounded-xl px-4 py-3">
          Per-project spend and token totals are rolled up from the Agent Platform when finance metadata is available on
          each project. To create, rename, delete, or switch the active project, use{' '}
          <Link to="/projects" className="font-bold text-darkDelegation hover:underline">
            Projects
          </Link>
          . Session-level estimates for the open project also appear on{' '}
          <Link to="/finance/project" className="font-bold text-emerald-700 hover:underline">
            Current project detail
          </Link>
          .
        </p>

        {loadError && <p className="text-sm text-red-600 mb-4">{loadError}</p>}

        {loading ? (
          <div className="flex items-center gap-2 text-zinc-500 text-sm">
            <Loader2 size={18} className="animate-spin" />
            Loading portfolio…
          </div>
        ) : (
          <>
            <div className="mt-2 flex flex-wrap gap-6 text-sm">
              <div className="bg-white border border-zinc-200 rounded-xl px-5 py-4 min-w-[200px]">
                <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-1">Portfolio tokens</p>
                <p className="text-2xl font-mono font-black text-darkDelegation">{formatTokens(totals.tokens)}</p>
              </div>
              <div className="bg-white border border-zinc-200 rounded-xl px-5 py-4 min-w-[200px]">
                <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-1">Portfolio est. spend</p>
                <p className="text-2xl font-mono font-black text-emerald-700">${totals.cost.toFixed(4)}</p>
              </div>
              <div className="bg-white border border-zinc-200 rounded-xl px-5 py-4 min-w-[200px] flex flex-col justify-center">
                <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-2">Projects in rollup</p>
                <p className="text-2xl font-black text-darkDelegation">{rows.length}</p>
              </div>
            </div>
          </>
        )}
      </main>
    </div>
  );
};
