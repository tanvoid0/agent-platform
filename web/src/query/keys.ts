import type { ProcessListProjectFilter } from "../api/types";

export type { ProcessListProjectFilter };

export const queryKeys = {
  processes: {
    all: ["processes"] as const,
    list: (limit?: number, projectFilter?: ProcessListProjectFilter) =>
      ["processes", "list", limit ?? "default", projectFilter ?? "all"] as const,
    detail: (id: number | null) => ["processes", "detail", id] as const,
    events: (id: number | null, eventFilter: string) =>
      ["processes", "events", id, eventFilter] as const,
  },
  teams: {
    all: ["teams"] as const,
    list: () => ["teams", "list"] as const,
    detail: (id: number | null) => ["teams", "detail", id] as const,
  },
  projects: {
    all: ["projects"] as const,
    list: () => ["projects", "list"] as const,
    detail: (id: number | null) => ["projects", "detail", id] as const,
  },
  modelOps: {
    all: ["modelOps"] as const,
    projects: () => ["modelOps", "projects"] as const,
    project: (name: string | null) => ["modelOps", "project", name] as const,
    job: (id: number | null) => ["modelOps", "job", id] as const,
    ollamaModels: () => ["modelOps", "ollamaModels"] as const,
    registry: () => ["modelOps", "registry"] as const,
  },
};
