import type { ProjectSideTab } from '@/interface/projectView/ProjectSideTabs';

const PROJECT_SIDE_TAB_KEY = 'ui:project-side-tab';

export function readStoredProjectSideTab(
  allowed: readonly ProjectSideTab[],
  fallback: ProjectSideTab,
): ProjectSideTab {
  try {
    const raw = localStorage.getItem(PROJECT_SIDE_TAB_KEY);
    if (raw && allowed.includes(raw as ProjectSideTab)) {
      return raw as ProjectSideTab;
    }
    if (raw === 'agents' && allowed.includes('overview')) {
      return 'overview';
    }
  } catch {
    /* ignore */
  }
  return fallback;
}

export function persistProjectSideTab(tab: ProjectSideTab): void {
  try {
    localStorage.setItem(PROJECT_SIDE_TAB_KEY, tab);
  } catch {
    /* ignore */
  }
}
