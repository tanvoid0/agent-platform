import { ArrowLeft, Boxes, Cpu, Image as ImageIcon, Palette } from 'lucide-react';
import React from 'react';
import { Link, NavLink, Outlet } from 'react-router-dom';

const navItems = [
  {
    to: 'ai',
    label: 'AI & models',
    hint: 'Chat path, keys, media defaults',
    Icon: Cpu,
  },
  {
    to: 'proxy',
    label: 'LLM proxy',
    hint: 'Server env & config.yaml',
    Icon: Boxes,
  },
  {
    to: 'assets',
    label: 'Asset defaults',
    hint: 'Image & video starting values',
    Icon: ImageIcon,
  },
  {
    to: 'scene',
    label: '3D office',
    hint: 'Look & performance',
    Icon: Palette,
  },
] as const;

export const SettingsLayout: React.FC = () => {
  return (
    <div className="min-h-screen bg-zinc-50 flex flex-col">
      <header className="border-b border-zinc-200 bg-white sticky top-0 z-20 shrink-0">
        <div className="max-w-5xl mx-auto px-4 sm:px-5 py-4 flex items-center gap-4">
          <Link
            to="/"
            className="flex items-center gap-2 text-zinc-500 hover:text-darkDelegation transition-colors text-xs font-black uppercase tracking-wider"
          >
            <ArrowLeft size={16} strokeWidth={2.5} />
            Workspace
          </Link>
          <span className="text-zinc-200">|</span>
          <h1 className="text-sm font-black text-darkDelegation uppercase tracking-widest">Settings</h1>
        </div>
      </header>

      <div className="flex-1 max-w-5xl mx-auto w-full flex flex-col md:flex-row min-h-0 min-w-0">
        <nav
          className="shrink-0 border-b md:border-b-0 md:border-r border-zinc-200 bg-white md:bg-zinc-50/80 px-4 sm:px-5 py-4 md:py-8 md:w-56 lg:w-60"
          aria-label="Settings sections"
        >
          <ul className="flex flex-row md:flex-col gap-1 overflow-x-auto md:overflow-visible pb-1 md:pb-0 -mx-1 px-1 md:mx-0 md:px-0">
            {navItems.map(({ to, label, hint, Icon }) => (
              <li key={to} className="shrink-0 md:shrink">
                <NavLink
                  to={to}
                  className={({ isActive }) =>
                    `flex items-start gap-2.5 rounded-xl px-3 py-2.5 text-left transition-colors md:w-full border ${
                      isActive
                        ? 'bg-white border-zinc-200 shadow-sm text-darkDelegation'
                        : 'border-transparent text-zinc-600 hover:bg-white/80 hover:border-zinc-200/80'
                    }`
                  }
                >
                  {({ isActive }) => (
                    <>
                      <Icon
                        className={`size-4 shrink-0 mt-0.5 ${isActive ? 'text-darkDelegation' : 'text-zinc-400'}`}
                        strokeWidth={2}
                        aria-hidden
                      />
                      <span className="min-w-0">
                        <span className="block text-[11px] font-black uppercase tracking-wide leading-tight">
                          {label}
                        </span>
                        <span className="hidden md:block text-[10px] text-zinc-400 font-medium leading-snug mt-0.5">
                          {hint}
                        </span>
                      </span>
                    </>
                  )}
                </NavLink>
              </li>
            ))}
          </ul>
        </nav>

        <main className="flex-1 min-w-0 flex flex-col px-4 sm:px-5 py-6 md:py-8 pb-12">
          <div className="flex-1 min-h-0">
            <Outlet />
          </div>
          <p className="text-center text-xs text-zinc-400 font-medium pt-10 mt-auto">
            Projects:{' '}
            <Link to="/projects" className="text-darkDelegation font-black hover:underline">
              list and manage
            </Link>
            {' · '}
            Usage caps and spend:{' '}
            <Link to="/finance" className="text-darkDelegation font-black hover:underline">
              Finance
            </Link>
          </p>
        </main>
      </div>
    </div>
  );
};
