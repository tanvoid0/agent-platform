/**
 * @license
 * SPDX-License-Identifier: Apache-2.0
 */

import React, { useEffect } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import App from './App';
import { hasRemoteProjectBackend, listRemoteProjects } from './integration/api/projectRemoteApi';
import { initProjectPersistence } from './integration/projectPersistence';
import {
  ensureLlmConnectivityPolling,
  stopLlmConnectivityPolling,
} from './integration/store/llmConnectivityStore';
import { useProjectsApiReachabilityStore } from './integration/store/projectsApiReachabilityStore';
import { BudgetExceededModal } from './interface/BudgetExceededModal';
import { ToastViewport } from './interface/components/ToastViewport';
import { FinancePortfolioPage } from './interface/FinancePortfolioPage';
import { FinanceProjectPage } from './interface/FinanceProjectPage';
import { ProjectsPage } from './interface/ProjectsPage';
import { SettingsPage } from './interface/settings/SettingsPage';
import NewProjectNameModal from './interface/NewProjectNameModal';
import { ProjectOutputPage } from './interface/ProjectOutputPage';
import { TeamManagementPage } from './interface/TeamManagementPage';

function PersistenceShell({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    return initProjectPersistence();
  }, []);

  useEffect(() => {
    ensureLlmConnectivityPolling();
    return () => stopLlmConnectivityPolling();
  }, []);

  useEffect(() => {
    const store = useProjectsApiReachabilityStore.getState();
    if (!hasRemoteProjectBackend()) {
      store.setDisabled();
      return;
    }
    let cancelled = false;
    const checkServer = async () => {
      if (cancelled) return;
      store.beginCheck();
      const t0 = performance.now();
      try {
        await listRemoteProjects(1, 0);
        if (cancelled) return;
        store.recordResult({ ok: true, latencyMs: Math.round(performance.now() - t0) });
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : String(e);
        store.recordResult({ ok: false, latencyMs: Math.round(performance.now() - t0), error: msg });
      }
    };
    void checkServer();
    const id = window.setInterval(() => void checkServer(), 15000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  return <>{children}</>;
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
          <Route path="/settings" element={<SettingsPage />} />
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
