import React from 'react';
import { AiClientsSettingsPanel } from './AiClientsSettingsPanel';
import { AssetDefaultsSection } from './AssetDefaultsSection';
import { LlmProxySettingsPanel } from './LlmProxySettingsPanel';
import { SceneAppearanceSection } from './SceneAppearanceSection';

const sectionShell = 'bg-white rounded-2xl border border-zinc-200/80 p-6 md:p-8 shadow-sm';

export const SettingsAiPage: React.FC = () => (
  <section className={sectionShell}>
    <AiClientsSettingsPanel variant="page" />
  </section>
);

export const SettingsProxyPage: React.FC = () => (
  <section className={sectionShell}>
    <LlmProxySettingsPanel />
  </section>
);

export const SettingsAssetsPage: React.FC = () => (
  <section className={sectionShell}>
    <AssetDefaultsSection />
  </section>
);

export const SettingsScenePage: React.FC = () => (
  <section className={sectionShell}>
    <SceneAppearanceSection />
  </section>
);
