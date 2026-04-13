import {
  useProjectsApiReachabilityStore,
  type ProjectsApiReachability,
} from '../store/projectsApiReachabilityStore';

export type ProjectsApiStatus = ProjectsApiReachability;

/** Subscribes to shared Agent Platform projects list reachability (single poller in AppRoutes). */
export function useProjectsApiStatus(): ProjectsApiStatus {
  return useProjectsApiReachabilityStore((s) => s.status);
}
