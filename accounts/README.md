# Accounts UI

Hosted at `/accounts` by `agent-platformd`. Same Portal login as the store apps.

```
pnpm install
pnpm build
```

That writes `accounts/dist`. Point the server at it with `AGENT_PLATFORM_ACCOUNTS_DIST` or keep the folder at `accounts/dist` relative to the process cwd.

If `dist` is missing, the server serves the embedded `accounts/index.html` fallback so `cargo run` still works.

v1 pages: sign-in (magic link), me (usage + billing), admin (grant-comp, set entitlement, revoke).
