import { ArrowLeft } from 'lucide-react';
import React from 'react';
import { Link } from 'react-router-dom';
import { AiClientsSettingsPanel } from './AiClientsSettingsPanel';
import { AssetDefaultsSection } from './AssetDefaultsSection';
import { SceneAppearanceSection } from './SceneAppearanceSection';

export const SettingsPage: React.FC = () => {
  return (
    <div className="min-h-screen bg-zinc-50 flex flex-col">
      <header className="border-b border-zinc-200 bg-white sticky top-0 z-20 shrink-0">
        <div className="max-w-3xl mx-auto px-5 py-4 flex items-center gap-4">
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

      <main className="flex-1 max-w-3xl mx-auto w-full px-5 py-8 space-y-8 pb-16">
        <section className="bg-white rounded-2xl border border-zinc-200/80 p-6 md:p-8 shadow-sm">
          <AiClientsSettingsPanel variant="page" />
        </section>

        <section className="bg-white rounded-2xl border border-zinc-200/80 p-6 md:p-8 shadow-sm">
          <AssetDefaultsSection />
        </section>

        <section className="bg-white rounded-2xl border border-zinc-200/80 p-6 md:p-8 shadow-sm">
          <SceneAppearanceSection />
        </section>

        <p className="text-center text-xs text-zinc-400 font-medium">
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
  );
};
