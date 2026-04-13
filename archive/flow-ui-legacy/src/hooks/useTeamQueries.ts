import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createTeam, deleteTeam, fetchTeamDetail, fetchTeamsList, updateTeam } from "../api/client";
import type { TeamRoster } from "../api/types";
import { queryKeys } from "../query/keys";

export function useTeamsListQuery() {
  return useQuery({
    queryKey: queryKeys.teams.list(),
    queryFn: fetchTeamsList,
    staleTime: 30_000,
  });
}

export function useTeamDetailQuery(teamId: number | null) {
  return useQuery({
    queryKey: queryKeys.teams.detail(teamId),
    queryFn: () => fetchTeamDetail(teamId!),
    enabled: teamId != null && teamId > 0,
  });
}

export function useCreateTeamMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      name: string;
      description?: string | null;
      color?: string | null;
      category?: string | null;
      roster: TeamRoster;
    }) => createTeam(body),
    onSuccess: (data) => {
      void qc.invalidateQueries({ queryKey: queryKeys.teams.all });
      void qc.invalidateQueries({ queryKey: queryKeys.teams.detail(data.id) });
    },
  });
}

export function useUpdateTeamMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (vars: {
      teamId: number;
      body: Partial<{
        name: string;
        description: string | null;
        color: string | null;
        category: string | null;
        roster: TeamRoster;
      }>;
    }) => updateTeam(vars.teamId, vars.body),
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: queryKeys.teams.all });
      void qc.invalidateQueries({ queryKey: queryKeys.teams.detail(vars.teamId) });
    },
  });
}

export function useDeleteTeamMutation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (teamId: number) => deleteTeam(teamId),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.teams.all });
    },
  });
}
