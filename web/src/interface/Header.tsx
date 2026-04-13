import { Bot, FolderKanban, Info, Loader2, Maximize2, Server, Settings, SlidersHorizontal, Wallet } from 'lucide-react';
import React, { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import packageJson from '../../package.json';
import { useHeaderByokUi } from '../integration/store/uiSelectors';
import { useConnectivityReadout } from '../integration/hooks/useConnectivityReadout';
import { useDelegationConnectivity } from '../integration/hooks/useDelegationConnectivity';
import { HeaderConnectivityReadout } from './components/HeaderConnectivityReadout';
import BYOKModal from './BYOKModal';
import InfoModal from './InfoModal';
import { ProjectSwitcher } from './ProjectSwitcher';
import { useProjectStatusBadge } from './projectView/useProjectStatusBadge';

const version = packageJson.version;

function trafficDotClass(kind: 'green' | 'yellow' | 'red', pulse = false): string {
  const pulseCls = pulse ? ' animate-pulse' : '';
  switch (kind) {
    case 'green':
      return 'bg-emerald-500 shadow-[0_0_0_1px_rgba(16,185,129,0.35)]';
    case 'yellow':
      return `bg-amber-400 shadow-[0_0_0_1px_rgba(251,191,36,0.45)${pulseCls}`;
    case 'red':
      return 'bg-red-500 shadow-[0_0_0_1px_rgba(239,68,68,0.35)]';
  }
}

const Header: React.FC = () => {
  const { isBYOKOpen, setBYOKOpen } = useHeaderByokUi();
  const [isInfoOpen, setIsInfoOpen] = useState(false);
  const { projectsTraffic, projectsTitle, llmTraffic, llmTitle } = useDelegationConnectivity();
  const { projects: projectsReadout, llm: llmReadout } = useConnectivityReadout();

  const byokTitle = useMemo(
    () => `${llmTitle} — click for AI settings\n${llmReadout.detail}`,
    [llmTitle, llmReadout.detail],
  );

  const { badge: projectStatusBadge } = useProjectStatusBadge();
  const projectIsActivelyWorking =
    projectStatusBadge.label === 'Working' && projectStatusBadge.pulse;

  const handleFullscreen = () => {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen();
    } else {
      if (document.exitFullscreen) {
        document.exitFullscreen();
      }
    }
  };

  return (
    <header className="h-14 border-b border-zinc-100 flex items-center justify-between px-6 bg-white shrink-0 relative z-40">
      {/* Left: Project Title */}
      <div className="flex items-center min-w-0">
        <img
          src="images/the-delegation.svg"
          alt="The Delegation"
          className="h-10 w-auto shrink-0"
        />

        <div className="flex items-center gap-3 self-start mt-3 ml-2 min-w-0 flex-wrap">
          <ProjectSwitcher />
          {projectIsActivelyWorking ? (
            <div
              className="flex items-center gap-1.5 rounded-full border border-darkDelegation/25 bg-zinc-50 px-2.5 py-1 shadow-sm ring-1 ring-darkDelegation/15"
              title="Agents are actively working (tasks in progress, asset generation, or model responding)"
              aria-live="polite"
            >
              <Loader2
                className="size-3.5 shrink-0 animate-spin text-darkDelegation"
                strokeWidth={2.5}
                aria-hidden
              />
              <span className="hidden text-[9px] font-black uppercase tracking-wider text-darkDelegation sm:inline">
                Working
              </span>
            </div>
          ) : null}
          <div className="flex items-center gap-1 shrink-0">
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              onClick={() => setIsInfoOpen(true)}
              className="text-zinc-300 hover:text-zinc-500"
              aria-label="About"
            >
              <Info size={14} strokeWidth={2} />
            </Button>
            <span className="text-[10px] font-medium text-zinc-400 font-mono">v{version}</span>
          </div>

          <div className="flex items-center gap-3 min-w-0">
            <a
              href="https://x.com/arturitu"
              target="_blank"
              rel="noopener"
              className="text-[10px] font-medium text-zinc-400 hover:text-darkDelegation transition-colors truncate"
            >
              @arturitu
            </a>
            <a
              href="https://github.com/arturitu/the-delegation"
              target="_blank"
              rel="noopener"
              className="text-zinc-300 hover:text-darkDelegation transition-colors shrink-0"
              title="View on GitHub"
            >
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"></path></svg>
            </a>
          </div>
        </div>
      </div>

      {/* Right: Global Controls */}
      <div className="flex items-center gap-3">
        <Link
          to="/projects"
          className="flex items-center gap-2 px-3 py-1 border border-zinc-200 hover:border-zinc-300 text-zinc-600 hover:text-darkDelegation rounded-lg transition-all h-9 shrink-0"
          title="All projects — create, rename, delete"
        >
          <FolderKanban size={14} />
          <span className="text-[10px] font-black uppercase tracking-wider hidden sm:inline">Projects</span>
        </Link>

        <Link
          to="/finance"
          className="flex items-center gap-2 px-3 py-1 border border-zinc-200 hover:border-zinc-300 text-zinc-600 hover:text-darkDelegation rounded-lg transition-all h-9 shrink-0"
          title="Finance & usage"
        >
          <Wallet size={14} />
          <span className="text-[10px] font-black uppercase tracking-wider hidden sm:inline">Finance</span>
        </Link>

        <Link
          to="/settings"
          className="flex items-center gap-2 px-3 py-1 border border-zinc-200 hover:border-zinc-300 text-zinc-600 hover:text-darkDelegation rounded-lg transition-all h-9 shrink-0"
          title="Settings — AI providers, models, 3D & defaults"
        >
          <SlidersHorizontal size={14} />
          <span className="text-[10px] font-black uppercase tracking-wider hidden sm:inline">Settings</span>
        </Link>

        <Link
          to="/teams"
          className="flex items-center gap-2 px-3 py-1 bg-darkDelegation hover:bg-darkDelegation text-white rounded-lg transition-all shadow-lg shadow-black/10 active:scale-95 h-9 shrink-0 ml-1"
          title="Manage Teams"
        >
          <Settings size={14} className="group-hover:rotate-45 transition-transform" />
          <span className="text-[10px] font-black uppercase tracking-wider ml-1 hidden sm:inline">Manage Teams</span>
        </Link>

        <HeaderConnectivityReadout projects={projectsReadout} llm={llmReadout} />

        <div className="w-px h-4 bg-zinc-200" />

        <div className="flex items-center gap-1.5 sm:gap-2">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={handleFullscreen}
            className="text-zinc-400 hover:text-darkDelegation"
            title="Fullscreen Browser"
          >
            <Maximize2 size={16} />
          </Button>

          {projectsTraffic !== 'off' && (
            <span
              className="relative inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg text-zinc-500"
              title={`${projectsTitle}\n${projectsReadout.detail}`}
              aria-label={`${projectsTitle}. ${projectsReadout.detail}`}
              role="img"
            >
              <Server size={17} strokeWidth={2} className="opacity-90" aria-hidden />
              <span
                className={`absolute bottom-1 right-1 size-2 rounded-full border-2 border-white ${trafficDotClass(projectsTraffic, projectsTraffic === 'yellow')}`}
                aria-hidden
              />
            </span>
          )}

          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => setBYOKOpen(true)}
            className="relative h-9 w-9 text-zinc-400 hover:text-darkDelegation"
            title={byokTitle}
            aria-label={byokTitle}
          >
            <Bot size={17} strokeWidth={2} className="opacity-90" />
            <span
              className={`absolute bottom-1 right-1 size-2 rounded-full border-2 border-white ${trafficDotClass(llmTraffic, llmTraffic === 'yellow')}`}
              aria-hidden
            />
          </Button>
        </div>
      </div>

      {isInfoOpen && (
        <InfoModal key="info-modal" onClose={() => setIsInfoOpen(false)} />
      )}

      {isBYOKOpen && (
        <BYOKModal key="byok-modal" onClose={() => setBYOKOpen(false)} />
      )}
    </header>
  );
};

export default Header;
