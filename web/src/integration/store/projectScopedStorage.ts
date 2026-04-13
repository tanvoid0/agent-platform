import type { StateStorage } from 'zustand/middleware';
import { getCachedProjectId } from '../api/projectRemoteApi';

function scopedKey(name: string): string {
  const pid = getCachedProjectId() || '__none__';
  return `${name}__proj__${pid}`;
}

/**
 * Namespaces `core-storage` by active project id so tasks/chats/logs do not bleed across projects.
 */
export function createProjectScopedCoreStorage(): StateStorage {
  return {
    getItem: (name) => {
      try {
        return localStorage.getItem(scopedKey(name));
      } catch {
        return null;
      }
    },
    setItem: (name, value) => {
      try {
        localStorage.setItem(scopedKey(name), value);
      } catch {
        /* ignore quota */
      }
    },
    removeItem: (name) => {
      try {
        localStorage.removeItem(scopedKey(name));
      } catch {
        /* ignore */
      }
    },
  };
}
