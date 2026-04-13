import { Navigate, Route, Routes, useParams } from "react-router-dom";
import { ProjectsPage } from "./components/ProjectsPage";
import { TeamsPage } from "./components/TeamsPage";
import { VIEW_MODE_PATH_SEGMENTS } from "./lib/processWorkspaceRoutes";
import { ProcessesPage } from "./pages/ProcessesPage";

/** Old bookmarks `/flow/runs/:id` → canonical `/flow/graph/:id`. */
function RedirectRunsToGraph() {
  const { legacyRunId } = useParams();
  return <Navigate to={legacyRunId != null ? `/graph/${legacyRunId}` : "/graph"} replace />;
}

export default function App() {
  return (
    <Routes>
      <Route index element={<Navigate to="/graph" replace />} />
      <Route path="runs" element={<Navigate to="/graph" replace />} />
      <Route path="runs/:legacyRunId" element={<RedirectRunsToGraph />} />
      {VIEW_MODE_PATH_SEGMENTS.map((v) => (
        <Route key={v} path={v} element={<ProcessesPage />} />
      ))}
      {VIEW_MODE_PATH_SEGMENTS.map((v) => (
        <Route key={`${v}-pid`} path={`${v}/:processId`} element={<ProcessesPage />} />
      ))}
      {/* Single route so :teamId is always the same param; avoids split route edge cases. */}
      <Route path="teams/:teamId?" element={<TeamsPage />} />
      <Route path="projects/:projectId?" element={<ProjectsPage />} />
      <Route path="*" element={<Navigate to="/graph" replace />} />
    </Routes>
  );
}
