# Accounts UI

`index.html` — one file, hash-routed, no build step. `agent-platformd` compiles
it in (`include_str!`) and serves it at `/accounts`, so the cloud image carries
no node toolchain.

Pages: sign-in (magic link), verify, me (usage, price preview, manage billing),
admin (grant-comp, set entitlement, revoke sessions). It talks to
`/accounts/api/v1/*` and keeps its tokens in `localStorage`.

There was a SvelteKit version of the same four screens beside it; it was deleted
rather than finished — two implementations of one admin page, and only this one
ships without `pnpm build`.
