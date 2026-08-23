# 14. User-owned data, local and cloud

Date: 2026-08-23

## Status

Accepted.

## Context

[ADR 0013](0013-desktop-local-open-cloud-account.md) left local loopback
open, like Ollama. That was the right call for *whether a Bearer token is
required on this machine*. It left two other questions unanswered, and both
showed up the moment another app pointed at the API or a second cloud account
signed in:

1. **Auth failures were opaque.** A hosted process with a master key answers
   `401 TOKEN_INVALID` / `"Missing or invalid Authorization (expected Bearer
   token)"` whether the header is absent, the JWT expired, or the token belongs
   to another install. Other apps see "the API suddenly stopped working" with
   nothing to branch on. `/health` said nothing about auth at all.
2. **Cloud data was not actually per-user.** Tenancy was workspace-token
   scoped. A Portal JWT has `workspace_id = None`, which every `assert_access`
   treated as the master key, so one signed-in user could list another user's
   projects. Coder threads, media jobs, workflows and action sets had no owner
   column at all.
3. **Local had no user row.** The same tables and the same `/me` shape could
   not be used for both installs without a second "no user" code path.

## Decision

**Identity is always a `users` row.** Local startup registers one from the
OS username (`kind = local`, email `local:{username}@localhost`) and gives it
a personal workspace. Cloud magic-link does the same for the email. Writes
stamp `user_id`; reads by a non-operator 404 (not 401) on someone else's row.

**Local loopback stays open** (ADR 0013). The change is that the unauthenticated
caller is that machine user, not `user_id = None`. Operator routes
(create-any-workspace, media, llm admin) still work for that process — it is
the person at the keyboard — but tenant lists are filtered to their row, which
after backfill is the whole local database.

**Cloud JWT is a tenant.** `AuthMode::UserSession` is not the master key.
`require_ai_entitlement` applies only to that mode; workspace tokens, the
master key, and the local machine user skip billing as they did when they had
no `user_id`.

**Auth errors name the failure.** Missing Bearer on a keyed server is
`AUTH_REQUIRED` (not `TOKEN_INVALID`). Expired JWTs are `TOKEN_EXPIRED` with
a refresh hint. `/health` (unauthenticated) carries `auth.required` / `auth.mode`
/ `auth.hint` so another app can see why `/api/v1/*` started 401ing.
`WWW-Authenticate: Bearer` is set on those responses.

## Consequences

- Schema: `users.username`, `users.kind`; `workspace.user_id`; `user_id` on
  coder / media / workflows / action_sets / search_history. Migration `0006`.
- `GET /api/v1/me` returns the same `{ user, workspace, auth }` object locally
  and in the cloud.
- Cross-user reads 404. The master key on a cloud process still sees every
  tenant (operator).
- A leftover keyed desktop daemon still gets a machine user so new rows are
  stamped; attach-if-running is unchanged.
