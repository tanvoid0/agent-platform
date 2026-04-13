import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
  type UseQueryOptions,
} from "@tanstack/react-query";
import {
  approveDag,
  cancelProcess,
  createProcess,
  fetchProcessDetail,
  fetchProcessEvents,
  fetchProcessesList,
  retryFailedTask,
  retryProcess,
  reviewTask,
  syncProcess,
} from "../api/client";
import type { ProcessDetailResponse, ProcessListProjectFilter, ReviewTaskBody } from "../api/types";
import { queryKeys } from "../query/keys";

const TERMINAL = new Set(["completed", "failed", "cancelled"]);

/** Matches `App.tsx` / backend: SSE stays open except for terminal states and human gates. */
export function processEligibleForEventStream(status: string | undefined): boolean {
  if (!status) return false;
  if (TERMINAL.has(status)) return false;
  if (status === "approval_required" || status === "task_review_required") return false;
  return true;
}

const POLL_MS_NO_SSE = 800;
const POLL_MS_SSE_BACKUP = 4000;
const POLL_MS_PROCESS_LIST = 3000;

function refetchIntervalForProcess(processStatus: string | undefined): number | false {
  if (!processStatus) return false;
  if (TERMINAL.has(processStatus)) return false;
  const sseActive = processEligibleForEventStream(processStatus);
  return sseActive ? POLL_MS_SSE_BACKUP : POLL_MS_NO_SSE;
}

export function useProcessesListQuery(
  limit = 50,
  projectFilter: ProcessListProjectFilter = "all",
  opts?: { refetchInterval?: number | false },
) {
  return useQuery({
    queryKey: queryKeys.processes.list(limit, projectFilter),
    queryFn: () => fetchProcessesList(limit, projectFilter),
    refetchInterval: opts?.refetchInterval ?? POLL_MS_PROCESS_LIST,
    refetchOnWindowFocus: true,
    refetchOnReconnect: true,
    placeholderData: keepPreviousData,
  });
}

export function useProcessEventsQuery(
  processId: number | null,
  eventFilter: string,
  processStatus: string | undefined,
) {
  const filter = eventFilter.trim() || "all";
  return useQuery({
    queryKey: queryKeys.processes.events(processId, filter),
    queryFn: () =>
      fetchProcessEvents(processId!, {
        eventType: filter === "all" ? undefined : filter,
        limit: 2000,
      }),
    enabled: processId != null && processId > 0,
    refetchOnWindowFocus: true,
    refetchOnReconnect: true,
    refetchInterval: refetchIntervalForProcess(processStatus),
    placeholderData: keepPreviousData,
  });
}

export function useProcessDetailQuery<TData = ProcessDetailResponse>(
  processId: number | null,
  options?: Omit<
    UseQueryOptions<ProcessDetailResponse, Error, TData>,
    "queryKey" | "queryFn" | "enabled"
  >,
) {
  return useQuery<ProcessDetailResponse, Error, TData>({
    queryKey: queryKeys.processes.detail(processId),
    queryFn: () => fetchProcessDetail(processId!),
    enabled: processId != null && processId > 0,
    refetchOnWindowFocus: true,
    refetchOnReconnect: true,
    refetchInterval: (q) => refetchIntervalForProcess(q.state.data?.process?.status),
    placeholderData: keepPreviousData,
    ...options,
  });
}

export function useProcessEventStreamEnabled(processId: number | null): boolean {
  const { data: ok } = useProcessDetailQuery(processId, {
    select: (d) => processEligibleForEventStream(d.process.status),
  });
  return processId != null && processId > 0 && !!ok;
}

export function useCreateProcessMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (vars: {
      goal: string;
      autoApprove?: boolean;
      teamTemplateId: number;
      projectId?: number | null;
    }) =>
      createProcess(vars.goal, {
        autoApprove: vars.autoApprove,
        teamTemplateId: vars.teamTemplateId,
        projectId: vars.projectId,
      }),
    onSuccess: (res) => {
      void qc.invalidateQueries({ queryKey: queryKeys.processes.all });
      void qc.invalidateQueries({ queryKey: queryKeys.projects.all });
      void qc.invalidateQueries({ queryKey: ["processes", "events", res.process_id] });
    },
  });
}

export function useApproveDagMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (vars: { processId: number; dagJson: string }) =>
      approveDag(vars.processId, vars.dagJson),
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: queryKeys.processes.detail(vars.processId) });
      void qc.invalidateQueries({ queryKey: ["processes", "events", vars.processId] });
      void qc.invalidateQueries({ queryKey: ["processes"] });
    },
  });
}

export function useCancelProcessMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (processId: number) => cancelProcess(processId),
    onSuccess: (_data, processId) => {
      void qc.invalidateQueries({ queryKey: queryKeys.processes.detail(processId) });
      void qc.invalidateQueries({ queryKey: ["processes", "events", processId] });
      void qc.invalidateQueries({ queryKey: ["processes"] });
    },
  });
}

export function useSyncProcessMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (processId: number) => syncProcess(processId),
    onSuccess: (_data, processId) => {
      void qc.invalidateQueries({ queryKey: queryKeys.processes.detail(processId) });
      void qc.invalidateQueries({ queryKey: ["processes", "events", processId] });
      void qc.invalidateQueries({ queryKey: ["processes"] });
    },
  });
}

export function useRetryProcessMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (processId: number) => retryProcess(processId),
    onSuccess: (_data, processId) => {
      void qc.invalidateQueries({ queryKey: queryKeys.processes.detail(processId) });
      void qc.invalidateQueries({ queryKey: ["processes", "events", processId] });
      void qc.invalidateQueries({ queryKey: ["processes"] });
    },
  });
}

export function useRetryTaskMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (vars: { processId: number; taskId: number }) =>
      retryFailedTask(vars.processId, vars.taskId),
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: queryKeys.processes.detail(vars.processId) });
      void qc.invalidateQueries({ queryKey: ["processes", "events", vars.processId] });
      void qc.invalidateQueries({ queryKey: ["processes"] });
    },
  });
}

export function useReviewTaskMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (vars: { processId: number; taskId: number } & ReviewTaskBody) => {
      const { processId, taskId, ...body } = vars;
      return reviewTask(processId, taskId, body);
    },
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: queryKeys.processes.detail(vars.processId) });
      void qc.invalidateQueries({ queryKey: ["processes", "events", vars.processId] });
      void qc.invalidateQueries({ queryKey: ["processes"] });
    },
  });
}
