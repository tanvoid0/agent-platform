# Project Scoping in Agent Platform API

## Overview

All processes and sessions in the Agent Platform must be associated with a project. Projects provide logical isolation and serve as the primary namespace for organizing agent workflows.

## Principles

1. **Every Process belongs to a Project** — Processes are scoped to projects for data organization and access control
2. **No Unscoped Queries** — API clients must explicitly filter by project, client_id, or request unassigned items only
3. **Frontend Always Knows Project** — When viewing a project in the UI, fetch only that project's processes

## Endpoints

### List Processes for a Project (Recommended)

```
GET /projects/{project_id}/processes?limit=50
```

**Use this when:**
- Viewing a project's workflow history in the UI
- Listing processes belonging to a specific project
- The project context is known

**Response:**
```json
{
  "processes": [
    {
      "id": 123,
      "goal": "Plan deployment",
      "status": "completed",
      "project_id": 42,
      ...
    }
  ]
}
```

### List Processes (Global, Filtered)

```
GET /processes?project_id={project_id}&limit=50
GET /processes?client_id={client_id}&limit=50
GET /processes?unassigned_only=true&limit=50
```

**Note:** At least one filter must be provided. Calling `/processes` without filters returns `400 Bad Request`.

| Parameter | Purpose |
|-----------|---------|
| `project_id` | Filter to processes in a specific project |
| `client_id` | Filter to processes for a specific external client |
| `unassigned_only` | Fetch unassigned processes (project_id is null) for manual assignment |

## Migration Guide

### For UI Clients

**Before:**
```javascript
// ❌ No filtering — gets all processes from all projects
const response = await fetch('/processes');
```

**After:**
```javascript
// ✅ Explicit project scoping
const projectId = getCurrentProjectId();
const response = await fetch(`/projects/${projectId}/processes`);
```

### For External Integrations

**Before:**
```javascript
// ❌ Implicit behavior, ambiguous scope
await fetch('/processes?limit=100');
```

**After:**
```javascript
// ✅ Explicit intent: get this client's processes
await fetch(`/processes?client_id=${clientId}&limit=100`);
```

## Why Project Scoping?

1. **Data Isolation** — Projects prevent accidental mixing of unrelated workflows
2. **Clarity** — Explicit filtering makes the API contract clear
3. **Performance** — Scoped queries are more efficient
4. **Correctness** — Impossible to accidentally fetch all data

## Breaking Changes

The `/processes` endpoint now requires at least one of:
- `project_id` parameter
- `client_id` parameter  
- `unassigned_only=true` parameter

Requests without a filter return `400 Bad Request: Must specify one of: project_id, client_id, or unassigned_only=true`.
