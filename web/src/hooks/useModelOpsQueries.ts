import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  createModelProject,
  fetchModelBuildJob,
  fetchModelProject,
  fetchModelProjects,
  fetchModelRegistry,
  fetchOllamaModels,
  pullOllamaModelAsync,
  startModelBuildJob,
} from "../api/modelOps";
import { queryKeys } from "../query/keys";

export function useModelProjectsQuery() {
  return useQuery({
    queryKey: queryKeys.modelOps.projects(),
    queryFn: fetchModelProjects,
    staleTime: 15_000,
  });
}

export function useModelProjectQuery(name: string | null) {
  return useQuery({
    queryKey: queryKeys.modelOps.project(name),
    queryFn: () => fetchModelProject(name!),
    enabled: name != null && name.length > 0,
  });
}

export function useModelBuildJobQuery(jobId: number | null, pollWhileRunning = true) {
  return useQuery({
    queryKey: queryKeys.modelOps.job(jobId),
    queryFn: () => fetchModelBuildJob(jobId!),
    enabled: jobId != null && jobId > 0,
    refetchInterval: (q) => {
      if (!pollWhileRunning) return false;
      const status = q.state.data?.status;
      if (status === "pending" || status === "running") return 2000;
      return false;
    },
  });
}

export function useOllamaModelsQuery() {
  return useQuery({
    queryKey: queryKeys.modelOps.ollamaModels(),
    queryFn: fetchOllamaModels,
    staleTime: 30_000,
  });
}

export function useModelRegistryQuery() {
  return useQuery({
    queryKey: queryKeys.modelOps.registry(),
    queryFn: fetchModelRegistry,
    staleTime: 15_000,
  });
}

export function useCreateModelProjectMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: createModelProject,
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.modelOps.all });
    },
  });
}

export function useStartModelBuildJobMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: startModelBuildJob,
    onSuccess: (job) => {
      void qc.invalidateQueries({ queryKey: queryKeys.modelOps.all });
      void qc.invalidateQueries({ queryKey: queryKeys.modelOps.job(job.id) });
    },
  });
}

export function usePullOllamaModelMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => pullOllamaModelAsync(name),
    onSuccess: (job) => {
      void qc.invalidateQueries({ queryKey: queryKeys.modelOps.ollamaModels() });
      void qc.invalidateQueries({ queryKey: queryKeys.modelOps.job(job.id) });
    },
  });
}
