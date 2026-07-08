import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createProject,
  deleteProject,
  fetchProjectDetail,
  fetchProjectsList,
  updateProject,
} from "../api/client";
import { queryKeys } from "../query/keys";

export function useProjectsListQuery() {
  return useQuery({
    queryKey: queryKeys.projects.list(),
    queryFn: fetchProjectsList,
    staleTime: 30_000,
  });
}

export function useProjectDetailQuery(projectId: number | null) {
  return useQuery({
    queryKey: queryKeys.projects.detail(projectId),
    queryFn: () => fetchProjectDetail(projectId!),
    enabled: projectId != null && projectId > 0,
  });
}

export function useCreateProjectMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; description?: string | null; color?: string | null }) =>
      createProject(body),
    onSuccess: (data) => {
      void qc.invalidateQueries({ queryKey: queryKeys.projects.all });
      void qc.invalidateQueries({ queryKey: queryKeys.projects.detail(data.id) });
    },
  });
}

export function useUpdateProjectMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (vars: {
      projectId: number;
      body: Partial<{ name: string; description: string | null; color: string | null }>;
    }) => updateProject(vars.projectId, vars.body),
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: queryKeys.projects.all });
      void qc.invalidateQueries({ queryKey: queryKeys.projects.detail(vars.projectId) });
    },
  });
}

export function useDeleteProjectMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (projectId: number) => deleteProject(projectId),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.projects.all });
    },
  });
}
