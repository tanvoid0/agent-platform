import { create } from 'zustand';

export type ProjectTitlePromptMode = 'simple' | 'newProject';

type PromptState = {
  open: boolean;
  headline: string;
  mode: ProjectTitlePromptMode;
};

export const useProjectTitlePromptStore = create<PromptState>(() => ({
  open: false,
  headline: '',
  mode: 'simple',
}));

/** Result of the new-project modal: consultant-first needs no name; named create carries `title`. */
export type NewProjectTitleResult =
  | { discussWithConsultant: true }
  | { discussWithConsultant: false; title: string };

let pendingSimpleResolve: ((s: string) => void) | null = null;
let pendingSimpleReject: (() => void) | null = null;

let pendingNewProjectResolve: ((r: NewProjectTitleResult) => void) | null = null;
let pendingNewProjectReject: (() => void) | null = null;

function abortError() {
  return new DOMException('User cancelled', 'AbortError');
}

/** Opens the global modal; resolves with trimmed title or rejects with AbortError if cancelled. */
export function requestProjectTitle(headline: string): Promise<string> {
  return new Promise((resolve, reject) => {
    pendingSimpleResolve = resolve;
    pendingSimpleReject = () => reject(abortError());
    pendingNewProjectResolve = null;
    pendingNewProjectReject = null;
    useProjectTitlePromptStore.setState({ open: true, headline, mode: 'simple' });
  });
}

/** New project flow: name + optional “Discuss with consultant”. */
export function requestNewProjectCreationTitle(headline: string): Promise<NewProjectTitleResult> {
  return new Promise((resolve, reject) => {
    pendingNewProjectResolve = resolve;
    pendingNewProjectReject = () => reject(abortError());
    pendingSimpleResolve = null;
    pendingSimpleReject = null;
    useProjectTitlePromptStore.setState({ open: true, headline, mode: 'newProject' });
  });
}

export function confirmProjectTitle(raw: string): void {
  const t = raw.trim();
  if (!t) return;
  const r = pendingSimpleResolve;
  pendingSimpleResolve = null;
  pendingSimpleReject = null;
  useProjectTitlePromptStore.setState({ open: false, headline: '', mode: 'simple' });
  r?.(t);
}

/** “Discuss with consultant” — no project name; API uses a placeholder until the user renames with help. */
export function confirmDiscussWithConsultantFirst(): void {
  const r = pendingNewProjectResolve;
  pendingNewProjectResolve = null;
  pendingNewProjectReject = null;
  useProjectTitlePromptStore.setState({ open: false, headline: '', mode: 'simple' });
  r?.({ discussWithConsultant: true });
}

export function confirmNewProjectWithName(raw: string): void {
  const t = raw.trim();
  if (!t) return;
  const r = pendingNewProjectResolve;
  pendingNewProjectResolve = null;
  pendingNewProjectReject = null;
  useProjectTitlePromptStore.setState({ open: false, headline: '', mode: 'simple' });
  r?.({ discussWithConsultant: false, title: t });
}

export function cancelProjectTitlePrompt(): void {
  const sr = pendingSimpleReject;
  const nr = pendingNewProjectReject;
  pendingSimpleResolve = null;
  pendingSimpleReject = null;
  pendingNewProjectResolve = null;
  pendingNewProjectReject = null;
  useProjectTitlePromptStore.setState({ open: false, headline: '', mode: 'simple' });
  sr?.();
  nr?.();
}

export function isProjectTitlePromptAbort(e: unknown): boolean {
  return e instanceof DOMException && e.name === 'AbortError';
}
