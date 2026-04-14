/**
 * @license
 * SPDX-License-Identifier: Apache-2.0
 */

import React, { useEffect } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import App from './App';
import { initProjectPersistence } from './integration/projectPersistence';
import { useProjectsReachabilityPolling } from './integration/hooks/useProjectsReachabilityPolling';
import { useChatPathStore } from './integration/store/chatPathStore';
import { useLlmUiCatalogStore } from './integration/store/llmUiCatalogStore';
import {
  ensureLlmConnectivityPolling,
  stopLlmConnectivityPolling,
} from './integration/store/llmConnectivityStore';
import { BudgetExceededModal } from './interface/BudgetExceededModal';
import { ToastViewport } from './interface/components/ToastViewport';
import { FinancePortfolioPage } from './interface/FinancePortfolioPage';
import { FinanceProjectPage } from './interface/FinanceProjectPage';
import { ProjectsPage } from './interface/ProjectsPage';
import { SettingsLayout } from './interface/settings/SettingsLayout';
import {
  SettingsAiPage,
  SettingsAssetsPage,
  SettingsProxyPage,
  SettingsScenePage,
} from './interface/settings/SettingsSectionPages';
import NewProjectNameModal from './interface/NewProjectNameModal';
import { ProjectOutputPage } from './interface/ProjectOutputPage';
import { TeamManagementPage } from './interface/TeamManagementPage';

function PersistenceShell({ children }: { children: React.ReactNode }) {
  const chatPathStatus = useChatPathStore((s) => s.status);
  const loadChatPath = useChatPathStore((s) => s.load);
  const loadLlmUiCatalog = useLlmUiCatalogStore((s) => s.load);

  useEffect(() => {
    return initProjectPersistence();
  }, []);

  useEffect(() => {
    void Promise.all([loadChatPath(), loadLlmUiCatalog()]).then(() => {
      ensureLlmConnectivityPolling();
    });
    return () => stopLlmConnectivityPolling();
  }, [loadChatPath, loadLlmUiCatalog]);
  useProjectsReachabilityPolling();

  return (
    <div className="contents" data-chat-path-status={chatPathStatus}>
      {children}
    </div>
  );
}

export const AppRoutes: React.FC = () => {
  return (
    <BrowserRouter basename="/flow">
      <PersistenceShell>
        <Routes>
          <Route path="/" element={<App />} />
          <Route path="/projects" element={<ProjectsPage />} />
          <Route path="/finance" element={<FinancePortfolioPage />} />
          <Route path="/finance/project" element={<FinanceProjectPage />} />
          <Route path="/settings" element={<SettingsLayout />}>
            <Route index element={<Navigate to="ai" replace />} />
            <Route path="ai" element={<SettingsAiPage />} />
            <Route path="proxy" element={<SettingsProxyPage />} />
            <Route path="assets" element={<SettingsAssetsPage />} />
            <Route path="scene" element={<SettingsScenePage />} />
            <Route path="*" element={<Navigate to="/settings/ai" replace />} />
          </Route>
          <Route path="/teams" element={<TeamManagementPage />} />
          <Route path="/project-output" element={<ProjectOutputPage />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
        <BudgetExceededModal />
        <NewProjectNameModal />
        <ToastViewport />
      </PersistenceShell>
    </BrowserRouter>
  );
};
