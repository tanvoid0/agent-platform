import { ArrowLeft, BarChart3, FolderOpen } from 'lucide-react';
import React, { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { getAllAgents } from '../data/agents';
import { useCoreStore } from '../integration/store/coreStore';
import { useActiveTeam } from '../integration/store/teamStore';
import { formatTokens } from './formatTokens';

function startOfDayUtc(ms: number): number {
  const d = new Date(ms);
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
}

export const FinanceProjectPage: React.FC = () => {
  const activeTeam = useActiveTeam();
  const agents = getAllAgents(activeTeam);

  const usageLedger = useCoreStore((s) => s.usageLedger);
  const actionLog = useCoreStore((s) => s.actionLog);
  const totalTokenUsage = useCoreStore((s) => s.totalTokenUsage);
  const totalEstimatedCost = useCoreStore((s) => s.totalEstimatedCost);
  const budgetLimitUsd = useCoreStore((s) => s.budgetLimitUsd);
  const setBudgetLimitUsd = useCoreStore((s) => s.setBudgetLimitUsd);
  const tasks = useCoreStore((s) => s.tasks);
  const finalOutput = useCoreStore((s) => s.finalOutput);
  const finalAssetType = useCoreStore((s) => s.finalAssetType);
  const finalAssetContent = useCoreStore((s) => s.finalAssetContent);
  const isGeneratingAsset = useCoreStore((s) => s.isGeneratingAsset);
  const referenceImages = useCoreStore((s) => s.referenceImages);
  const phase = useCoreStore((s) => s.phase);

  const [budgetDraft, setBudgetDraft] = useState(() =>
    budgetLimitUsd != null && budgetLimitUsd > 0 ? String(budgetLimitUsd) : '',
  );
  const [ledgerPage, setLedgerPage] = useState(1);
  const [activityPage, setActivityPage] = useState(1);
  const LEDGER_PAGE_SIZE = 12;
  const ACTIVITY_PAGE_SIZE = 10;

  React.useEffect(() => {
    setBudgetDraft(budgetLimitUsd != null && budgetLimitUsd > 0 ? String(budgetLimitUsd) : '');
  }, [budgetLimitUsd]);

  const ledgerDesc = useMemo(
    () => [...usageLedger].sort((a, b) => b.timestamp - a.timestamp),
    [usageLedger],
  );
  const activityDesc = useMemo(
    () => [...actionLog].sort((a, b) => b.timestamp - a.timestamp),
    [actionLog],
  );

  const byModel = useMemo(() => {
    const m = new Map<string, { tokens: number; cost: number }>();
    for (const row of usageLedger) {
      const key = row.model || 'unknown';
      const cur = m.get(key) ?? { tokens: 0, cost: 0 };
      cur.tokens += row.totalTokens;
      cur.cost += row.estimatedCostUsd;
      m.set(key, cur);
    }
    return [...m.entries()].sort((a, b) => b[1].cost - a[1].cost);
  }, [usageLedger]);

  const byAgent = useMemo(() => {
    const m = new Map<number, { name: string; tokens: number; cost: number }>();
    for (const row of usageLedger) {
      const cur = m.get(row.agentIndex) ?? { name: row.agentName, tokens: 0, cost: 0 };
      cur.tokens += row.totalTokens;
      cur.cost += row.estimatedCostUsd;
      cur.name = row.agentName;
      m.set(row.agentIndex, cur);
    }
    return [...m.entries()].sort((a, b) => b[1].cost - a[1].cost);
  }, [usageLedger]);

  const maxAgentCost = byAgent[0]?.[1].cost || 1;

  const last7Days = useMemo(() => {
    const now = Date.now();
    const dayMs = 86400000;
    const startToday = startOfDayUtc(now);
    const buckets: { label: string; cost: number; tokens: number }[] = [];
    for (let i = 6; i >= 0; i--) {
      const dayStart = startToday - i * dayMs;
      const label = new Date(dayStart).toISOString().slice(5, 10);
      buckets.push({ label, cost: 0, tokens: 0 });
    }
    for (const row of usageLedger) {
      const idx = Math.floor((startOfDayUtc(row.timestamp) - (startToday - 6 * dayMs)) / dayMs);
      if (idx >= 0 && idx < 7) {
        buckets[idx].cost += row.estimatedCostUsd;
        buckets[idx].tokens += row.totalTokens;
      }
    }
    return buckets;
  }, [usageLedger]);

  const maxDayCost = Math.max(...last7Days.map((d) => d.cost), 0.0001);
  const totalLedgerPages = Math.max(1, Math.ceil(ledgerDesc.length / LEDGER_PAGE_SIZE));
  const totalActivityPages = Math.max(1, Math.ceil(activityDesc.length / ACTIVITY_PAGE_SIZE));
  const safeLedgerPage = Math.min(ledgerPage, totalLedgerPages);
  const safeActivityPage = Math.min(activityPage, totalActivityPages);
  const ledgerRows = ledgerDesc.slice(
    (safeLedgerPage - 1) * LEDGER_PAGE_SIZE,
    safeLedgerPage * LEDGER_PAGE_SIZE,
  );
  const activityRows = activityDesc.slice(
    (safeActivityPage - 1) * ACTIVITY_PAGE_SIZE,
    safeActivityPage * ACTIVITY_PAGE_SIZE,
  );
  const uniqueActivityAgents = new Set(actionLog.map((row) => row.agentIndex)).size;
  const doneTasks = tasks.filter((task) => task.status === 'done').length;
  const lastActivityAt = activityDesc[0]?.timestamp ?? null;
  const finalOutputPreview = (finalOutput || '').trim().slice(0, 320);
  const finalOutputWordCount = (finalOutput || '').trim()
    ? (finalOutput || '').trim().split(/\s+/).length
    : 0;
  const hasDeliverableAsset = Boolean(finalAssetContent);

  const applyBudget = () => {
    const t = budgetDraft.trim();
    if (!t) {
      setBudgetLimitUsd(null);
      return;
    }
    const n = Number(t);
    if (!Number.isFinite(n) || n <= 0) {
      setBudgetLimitUsd(null);
      return;
    }
    setBudgetLimitUsd(n);
  };

  const budgetCap = budgetLimitUsd != null && budgetLimitUsd > 0 ? budgetLimitUsd : null;
  const pct = budgetCap ? Math.min(100, (totalEstimatedCost / budgetCap) * 100) : 0;
  const over = budgetCap != null && totalEstimatedCost >= budgetCap;

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
          <Link
            to="/projects"
            className="text-xs font-bold text-zinc-500 hover:text-darkDelegation uppercase tracking-wide"
          >
            All projects
          </Link>
        </div>
        <h1 className="text-sm font-black text-darkDelegation uppercase tracking-widest">Current project</h1>
      </header>

      <main className="flex-1 max-w-5xl w-full mx-auto px-4 py-8 space-y-8">
        <section className="bg-white border border-zinc-200 rounded-xl p-5 shadow-sm">
          <h2 className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-4 flex items-center gap-2">
            <BarChart3 size={14} />
            Totals
          </h2>
          <div className="flex flex-wrap gap-8">
            <div>
              <p className="text-[10px] font-bold text-zinc-400 uppercase mb-1">Tokens</p>
              <p className="text-3xl font-mono font-black text-darkDelegation">{formatTokens(totalTokenUsage.totalTokens)}</p>
              <p className="text-xs text-zinc-500 font-mono mt-1">
                {formatTokens(totalTokenUsage.promptTokens)} in + {formatTokens(totalTokenUsage.completionTokens)} out
              </p>
            </div>
            <div>
              <p className="text-[10px] font-bold text-zinc-400 uppercase mb-1">Estimated cost</p>
              <p className="text-3xl font-mono font-black text-emerald-700">${totalEstimatedCost.toFixed(4)}</p>
            </div>
          </div>
        </section>

        <section className="bg-white border border-zinc-200 rounded-xl p-5 shadow-sm">
          <div className="mb-4 flex items-center justify-between gap-3">
            <h2 className="text-[10px] font-black uppercase tracking-widest text-zinc-400">
              Deliverables & final output
            </h2>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-8 gap-1.5 text-[9px] font-black uppercase tracking-wider"
              asChild
            >
              <Link to="/project-output">
                <FolderOpen size={13} />
                Open full output
              </Link>
            </Button>
          </div>

          <div className="grid gap-2 rounded-lg border border-zinc-100 bg-zinc-50/70 p-2.5 sm:grid-cols-2 lg:grid-cols-4">
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Project phase</p>
              <p className="text-[11px] font-semibold capitalize text-zinc-700">{phase}</p>
            </div>
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Deliverable type</p>
              <p className="text-[11px] font-semibold uppercase text-zinc-700">{finalAssetType}</p>
            </div>
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Asset status</p>
              <p className="text-[11px] font-semibold text-zinc-700">
                {isGeneratingAsset ? 'Generating' : hasDeliverableAsset ? 'Generated' : 'Not generated'}
              </p>
            </div>
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Reference images</p>
              <p className="text-[11px] font-semibold text-zinc-700">{referenceImages.length}</p>
            </div>
          </div>

          <div className="mt-3 rounded-lg border border-zinc-100 bg-white p-3">
            <div className="mb-2 flex items-center justify-between gap-2">
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Final output preview</p>
              <p className="text-[10px] font-mono text-zinc-400">{finalOutputWordCount} words</p>
            </div>
            {finalOutputPreview ? (
              <p className="text-xs leading-relaxed text-zinc-700">
                {finalOutputPreview}
                {(finalOutput || '').trim().length > finalOutputPreview.length ? '…' : ''}
              </p>
            ) : (
              <p className="text-xs text-zinc-500">No final output saved yet.</p>
            )}
          </div>
        </section>

        <section className="bg-white border border-zinc-200 rounded-xl p-5 shadow-sm">
          <h2 className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-4">Budget cap (cloud / Gemini)</h2>
          <p className="text-sm text-zinc-600 mb-4">
            When set, new cloud (Gemini) chat completions and final media generation are blocked if estimated spend is already at or above this USD cap. Server-routed chat is not blocked.
          </p>
          <div className="flex flex-wrap items-end gap-3 mb-4">
            <div>
              <label className="block text-[10px] font-bold text-zinc-500 uppercase mb-1">Limit (USD)</label>
              <Input
                type="number"
                min={0}
                step={0.01}
                placeholder="No cap"
                value={budgetDraft}
                onChange={(e) => setBudgetDraft(e.target.value)}
                className="w-40 rounded-lg border-zinc-200 px-3 py-2 font-mono text-sm"
              />
            </div>
            <Button
              type="button"
              onClick={applyBudget}
              className="rounded-lg bg-darkDelegation px-4 py-2 text-xs font-black uppercase tracking-wide text-white hover:opacity-95"
            >
              Save
            </Button>
          </div>
          {budgetCap != null && (
            <div>
              <div className="flex justify-between text-xs font-mono mb-1">
                <span className={over ? 'text-red-600 font-bold' : 'text-zinc-600'}>
                  {over ? 'At or over cap' : `${pct.toFixed(0)}% of cap`}
                </span>
                <span className="text-zinc-500">
                  ${totalEstimatedCost.toFixed(4)} / ${budgetCap.toFixed(2)}
                </span>
              </div>
              <div className="h-2 bg-zinc-100 rounded-full overflow-hidden">
                <div
                  className={`h-full rounded-full transition-all ${over ? 'bg-red-500' : 'bg-emerald-500'}`}
                  style={{ width: `${pct}%` }}
                />
              </div>
            </div>
          )}
        </section>

        <div className="grid md:grid-cols-2 gap-6">
          <section className="bg-white border border-zinc-200 rounded-xl p-5 shadow-sm">
            <h3 className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-4">By agent</h3>
            <div className="space-y-3">
              {byAgent.length === 0 ? (
                <p className="text-sm text-zinc-500">No ledger entries yet.</p>
              ) : (
                byAgent.map(([idx, v]) => {
                  const agent = agents.find((a) => a.index === idx);
                  const label = agent?.name ?? v.name;
                  const w = (v.cost / maxAgentCost) * 100;
                  return (
                    <div key={idx}>
                      <div className="flex justify-between text-xs mb-1">
                        <span className="font-bold text-zinc-700 truncate">{label}</span>
                        <span className="font-mono text-emerald-700 shrink-0">${v.cost.toFixed(4)}</span>
                      </div>
                      <div className="h-1.5 bg-zinc-100 rounded-full overflow-hidden">
                        <div className="h-full bg-zinc-400 rounded-full" style={{ width: `${w}%` }} />
                      </div>
                      <p className="text-[10px] text-zinc-400 font-mono mt-0.5">{formatTokens(v.tokens)} tok</p>
                    </div>
                  );
                })
              )}
            </div>
          </section>

          <section className="bg-white border border-zinc-200 rounded-xl p-5 shadow-sm">
            <h3 className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-4">By model</h3>
            <div className="space-y-2 max-h-48 overflow-y-auto">
              {byModel.length === 0 ? (
                <p className="text-sm text-zinc-500">No ledger entries yet.</p>
              ) : (
                byModel.map(([name, v]) => (
                  <div key={name} className="flex justify-between text-xs border-b border-zinc-50 pb-2">
                    <span className="font-mono text-zinc-700 truncate mr-2">{name}</span>
                    <span className="font-mono text-emerald-700 shrink-0">${v.cost.toFixed(4)}</span>
                  </div>
                ))
              )}
            </div>
          </section>
        </div>

        <section className="bg-white border border-zinc-200 rounded-xl p-5 shadow-sm">
          <h3 className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-4">Last 7 days (est. cost)</h3>
          <div className="flex gap-2 h-32 items-stretch">
            {last7Days.map((d) => {
              const barPct = (d.cost / maxDayCost) * 100;
              const barH = Math.max(2, Math.round((barPct / 100) * 112));
              return (
                <div key={d.label} className="flex-1 flex flex-col items-center gap-1 min-w-0 justify-end">
                  <div className="w-full flex-1 flex flex-col justify-end min-h-0">
                    <div
                      className="w-full bg-emerald-200 rounded-t transition-all"
                      style={{ height: barH }}
                      title={`$${d.cost.toFixed(4)}`}
                    />
                  </div>
                  <span className="text-[9px] font-mono text-zinc-500 shrink-0">{d.label}</span>
                </div>
              );
            })}
          </div>
        </section>

        <section className="bg-white border border-zinc-200 rounded-xl overflow-hidden shadow-sm">
          <h3 className="text-[10px] font-black uppercase tracking-widest text-zinc-400 px-5 pt-5 pb-2">Usage ledger</h3>
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead>
                <tr className="border-y border-zinc-100 bg-zinc-50/80">
                  <th className="px-4 py-2 font-black uppercase text-zinc-500">Time</th>
                  <th className="px-4 py-2 font-black uppercase text-zinc-500">Agent</th>
                  <th className="px-4 py-2 font-black uppercase text-zinc-500">Kind</th>
                  <th className="px-4 py-2 font-black uppercase text-zinc-500">Model</th>
                  <th className="px-4 py-2 font-black uppercase text-zinc-500 text-right">In</th>
                  <th className="px-4 py-2 font-black uppercase text-zinc-500 text-right">Out</th>
                  <th className="px-4 py-2 font-black uppercase text-zinc-500 text-right">Cost</th>
                </tr>
              </thead>
              <tbody>
                {ledgerRows.map((row) => (
                  <tr key={row.id} className="border-b border-zinc-50 hover:bg-zinc-50/50">
                    <td className="px-4 py-2 font-mono text-zinc-600 whitespace-nowrap">
                      {new Date(row.timestamp).toLocaleString()}
                    </td>
                    <td className="px-4 py-2 font-bold text-zinc-700">{row.agentName}</td>
                    <td className="px-4 py-2 text-zinc-600">{row.kind}</td>
                    <td className="px-4 py-2 font-mono text-zinc-600 truncate max-w-[140px]" title={row.model}>
                      {row.model}
                    </td>
                    <td className="px-4 py-2 text-right font-mono">{formatTokens(row.promptTokens)}</td>
                    <td className="px-4 py-2 text-right font-mono">{formatTokens(row.completionTokens)}</td>
                    <td className="px-4 py-2 text-right font-mono text-emerald-700">${row.estimatedCostUsd.toFixed(4)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {ledgerDesc.length > 0 && (
            <div className="flex items-center justify-between border-t border-zinc-100 px-4 py-3">
              <p className="text-[10px] text-zinc-500">
                Showing {(safeLedgerPage - 1) * LEDGER_PAGE_SIZE + 1}-
                {Math.min(safeLedgerPage * LEDGER_PAGE_SIZE, ledgerDesc.length)} of {ledgerDesc.length}
              </p>
              <div className="flex items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={safeLedgerPage <= 1}
                  onClick={() => setLedgerPage((p) => Math.max(1, p - 1))}
                  className="h-7 px-2 text-[9px] font-bold uppercase"
                >
                  Prev
                </Button>
                <span className="text-[10px] font-mono text-zinc-500">
                  {safeLedgerPage}/{totalLedgerPages}
                </span>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={safeLedgerPage >= totalLedgerPages}
                  onClick={() => setLedgerPage((p) => Math.min(totalLedgerPages, p + 1))}
                  className="h-7 px-2 text-[9px] font-bold uppercase"
                >
                  Next
                </Button>
              </div>
            </div>
          )}
          {ledgerDesc.length === 0 && (
            <p className="px-5 py-8 text-sm text-zinc-500 text-center">No API calls with usage logged yet.</p>
          )}
        </section>

        <section className="bg-white border border-zinc-200 rounded-xl overflow-hidden shadow-sm">
          <h3 className="text-[10px] font-black uppercase tracking-widest text-zinc-400 px-5 pt-5 pb-4">
            Activity log
          </h3>
          <div className="grid gap-2 border-y border-zinc-100 bg-zinc-50/70 px-4 py-3 sm:grid-cols-4">
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Events</p>
              <p className="text-sm font-black text-darkDelegation">{actionLog.length}</p>
            </div>
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Agents active</p>
              <p className="text-sm font-black text-darkDelegation">{uniqueActivityAgents}</p>
            </div>
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Tasks done</p>
              <p className="text-sm font-black text-darkDelegation">
                {doneTasks}/{tasks.length}
              </p>
            </div>
            <div>
              <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Last event</p>
              <p className="truncate text-[11px] font-mono font-semibold text-zinc-600">
                {lastActivityAt ? new Date(lastActivityAt).toLocaleString() : '—'}
              </p>
            </div>
          </div>
          <div className="max-h-[460px] overflow-auto">
            {activityRows.length > 0 ? (
              <ul className="divide-y divide-zinc-100">
                {activityRows.map((row) => {
                  const agent = agents.find((a) => a.index === row.agentIndex);
                  const agentName = agent?.name ?? `Agent ${row.agentIndex}`;
                  return (
                    <li key={row.id} className="px-4 py-3">
                      <div className="mb-1 flex items-center justify-between gap-2">
                        <p className="truncate text-[11px] font-black uppercase tracking-wide text-zinc-600">
                          {agentName}
                        </p>
                        <p className="shrink-0 text-[10px] font-mono text-zinc-400">
                          {new Date(row.timestamp).toLocaleString()}
                        </p>
                      </div>
                      <p className="text-xs leading-relaxed text-zinc-700">{row.action}</p>
                      {row.taskId ? (
                        <p className="mt-1 text-[10px] font-mono text-zinc-400">Task: {row.taskId}</p>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            ) : (
              <p className="px-5 py-8 text-sm text-zinc-500 text-center">No project activity logged yet.</p>
            )}
          </div>
          {activityDesc.length > 0 && (
            <div className="flex items-center justify-between border-t border-zinc-100 px-4 py-3">
              <p className="text-[10px] text-zinc-500">
                Showing {(safeActivityPage - 1) * ACTIVITY_PAGE_SIZE + 1}-
                {Math.min(safeActivityPage * ACTIVITY_PAGE_SIZE, activityDesc.length)} of {activityDesc.length}
              </p>
              <div className="flex items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={safeActivityPage <= 1}
                  onClick={() => setActivityPage((p) => Math.max(1, p - 1))}
                  className="h-7 px-2 text-[9px] font-bold uppercase"
                >
                  Prev
                </Button>
                <span className="text-[10px] font-mono text-zinc-500">
                  {safeActivityPage}/{totalActivityPages}
                </span>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={safeActivityPage >= totalActivityPages}
                  onClick={() => setActivityPage((p) => Math.min(totalActivityPages, p + 1))}
                  className="h-7 px-2 text-[9px] font-bold uppercase"
                >
                  Next
                </Button>
              </div>
            </div>
          )}
        </section>

      </main>
    </div>
  );
};
