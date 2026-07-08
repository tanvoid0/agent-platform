import { MenuIcon } from "lucide-react";
import { NavLink, useLocation } from "react-router-dom";

import { processEligibleForEventStream, useProcessDetailQuery } from "../hooks/useProcessQueries";
import { isProcessWorkspacePath } from "../lib/processWorkspaceRoutes";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { CardDescription, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

export type AppTopNavProps = {
  committedProcessId: number | null;
  className?: string;
};

function TopNavProcessStatusBadge({ processId }: { processId: number }) {
  const { data: status, isPending } = useProcessDetailQuery(processId, {
    select: (d) => d.process.status,
  });
  const label = status ?? (isPending ? "…" : null);
  if (label == null) return null;
  return (
    <Badge
      variant="outline"
      className="max-w-[9rem] min-w-[4.5rem] justify-center truncate sm:max-w-[14rem]"
    >
      {label}
    </Badge>
  );
}

function TopNavSseBadge({ processId }: { processId: number }) {
  const { data: live } = useProcessDetailQuery(processId, {
    select: (d) => processEligibleForEventStream(d.process.status),
  });
  return live ? (
    <Badge variant="default" className="hidden sm:inline-flex">
      SSE live
    </Badge>
  ) : null;
}

const navLinkClass = ({ isActive }: { isActive: boolean }) =>
  cn(
    buttonVariants({ variant: isActive ? "default" : "ghost", size: "sm" }),
    "h-7 px-2.5 text-xs",
  );

export function AppTopNav({ committedProcessId, className }: AppTopNavProps) {
  const location = useLocation();
  const isProcessWorkspace = isProcessWorkspacePath(location.pathname);
  const isTeams = location.pathname === "/teams" || location.pathname.startsWith("/teams/");
  const isProjects = location.pathname === "/projects" || location.pathname.startsWith("/projects/");

  const title = isTeams ? "Teams" : isProjects ? "Projects" : "Team / DAG";
  const description = isTeams
    ? "Templates for planner roster hints"
    : isProjects
      ? "Group processes into workspaces"
      : "React Flow · TanStack Query + SSE";

  return (
    <header
      className={cn(
        "sticky top-0 z-50 border-b border-border bg-background/95 backdrop-blur-md supports-backdrop-filter:bg-background/80",
        className,
      )}
    >
      <div className="flex min-h-14 flex-wrap items-center gap-x-3 gap-y-2 px-4 py-2 sm:flex-nowrap sm:items-stretch sm:px-5 sm:py-0">
        <div className="flex min-w-0 max-w-[min(100%,16rem)] shrink-0 flex-col justify-center gap-0.5 py-1 sm:py-0">
          <CardTitle className="truncate text-sm leading-tight sm:text-base">{title}</CardTitle>
          <CardDescription className="truncate text-xs leading-tight">{description}</CardDescription>
        </div>

        <Separator
          orientation="vertical"
          className="hidden h-7 self-center sm:block sm:h-8"
        />

        <div className="flex w-full min-w-0 flex-[1_1_100%] flex-wrap items-center justify-start gap-2 sm:order-none sm:flex-[1_1_auto] sm:justify-center">
          <div className="flex shrink-0 items-center gap-1 rounded-lg border border-border bg-muted/40 p-0.5">
            <NavLink
              to="/graph"
              className={() => navLinkClass({ isActive: isProcessWorkspace })}
            >
              Processes
            </NavLink>
            <NavLink to="/teams" className={navLinkClass}>
              Teams
            </NavLink>
            <NavLink to="/projects" className={navLinkClass}>
              Projects
            </NavLink>
          </div>

          {isProcessWorkspace ? (
            <TabsList
              variant="line"
              aria-label="Task layout"
              className="h-auto min-h-8 w-full max-w-full flex-wrap justify-start gap-0 bg-transparent p-0 sm:w-auto sm:max-w-none sm:flex-nowrap sm:justify-center"
            >
              <TabsTrigger value="graph" className="px-3 py-1.5">
                Graph
              </TabsTrigger>
              <TabsTrigger value="board" className="px-3 py-1.5">
                Board
              </TabsTrigger>
              <TabsTrigger value="timeline" className="px-3 py-1.5">
                Timeline
              </TabsTrigger>
              <TabsTrigger value="events" className="px-3 py-1.5">
                Events
              </TabsTrigger>
            </TabsList>
          ) : null}
        </div>

        <div className="ml-auto flex shrink-0 flex-wrap items-center justify-end gap-2 sm:ml-0 sm:justify-normal">
          {committedProcessId != null && (
            <Badge variant="secondary" className="tabular-nums">
              #{committedProcessId}
            </Badge>
          )}
          {committedProcessId != null && committedProcessId > 0 ? (
            <>
              <TopNavProcessStatusBadge processId={committedProcessId} />
              <TopNavSseBadge processId={committedProcessId} />
            </>
          ) : null}

          <Separator
            orientation="vertical"
            className="hidden h-7 self-center md:block md:h-8"
          />

          <div className="hidden items-center gap-1.5 md:flex">
            <Badge variant="outline" render={<a href="/ui" />}>
              Minimal UI
            </Badge>
            <Badge variant="outline" render={<a href="/docs" />}>
              API
            </Badge>
          </div>

          <Sheet>
            <SheetTrigger
              className={cn(
                buttonVariants({ variant: "outline", size: "icon-sm" }),
                "md:hidden",
              )}
              aria-label="Open menu"
            >
              <MenuIcon />
            </SheetTrigger>
            <SheetContent side="right" className="gap-0 sm:max-w-xs">
              <SheetHeader>
                <SheetTitle>Links</SheetTitle>
                <SheetDescription>Other entry points for this stack.</SheetDescription>
              </SheetHeader>
              <div className="flex flex-col gap-2 px-4 pb-4">
                <Button variant="secondary" nativeButton={false} render={<a href="/ui" />}>
                  Minimal UI
                </Button>
                <Button variant="secondary" nativeButton={false} render={<a href="/docs" />}>
                  API docs
                </Button>
              </div>
            </SheetContent>
          </Sheet>
        </div>
      </div>
    </header>
  );
}
