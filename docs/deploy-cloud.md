# Cloud deploy

The API process stays on loopback. TLS and bots sit in front.

## Required env (public URL)

```
AGENT_PLATFORM_HOST=127.0.0.1
AGENT_PLATFORM_PORT=18410
AGENT_PLATFORM_MASTER_KEY=          # required off-loopback; required here too
AGENT_PLATFORM_JWT_SECRET=          # long random
AGENT_PLATFORM_PUBLIC_URL=https://api.example.com
AGENT_PLATFORM_ADMIN_EMAILS=you@example.com
STRIPE_SECRET_KEY=
STRIPE_WEBHOOK_SECRET=
AGENT_PLATFORM_PRICE_US=
AGENT_PLATFORM_PRICE_GB=
AGENT_PLATFORM_PRICE_BD=
AGENT_PLATFORM_PRICE_ROW=
```

Do **not** set `AGENT_PLATFORM_ALLOW_OPEN`.

## Caddy

See [Caddyfile](../Caddyfile). Point DNS at the box. Caddy gets the cert.

```
caddy run --config Caddyfile
```

`agent-platformd` binds `127.0.0.1:18410`. Caddy proxies `https://api.example.com` there.

## Cloudflare

Put Cloudflare in front of the same hostname if you want DDoS / bot fight.

- **Bot Fight / managed challenge:** `/accounts` and `/accounts/api/v1/auth/magic-link` only.
- **Do not** put Bot Fight on `/v1/*`. Native apps (Tauri, Android) will fail the challenge.
- Webhook path `/accounts/api/v1/billing/webhook` must skip browser challenges (Stripe servers).

WAF: rate-limit `/accounts/api/v1/auth/magic-link` if you want a second layer; the process already has `AGENT_PLATFORM_MAGIC_LINK_IP_RPM`.

## Stripe

1. Create four recurring Prices (US, GB, BD-lower-USD, ROW). Different amounts — not FX of one USD price.
2. Paste Price IDs into env.
3. Checkout webhook: `checkout.session.completed`, `invoice.paid`, `invoice.payment_failed`, `customer.subscription.deleted` → `https://api.example.com/accounts/api/v1/billing/webhook`.
4. Enable Stripe Tax. Do not bake VAT into the catalog amounts.
5. Customer Portal: enable in the Stripe dashboard.

**Done when:** a test US card + BD billing address charges the US Price ID.

## Store builds

Point portal-desktop / Equalizer / Android at `AGENT_PLATFORM_PUBLIC_URL`. They ship that URL and the public app id only — never a master key or `agp_` token.

Comp your own email:

```
agent-platformd grant-comp you@example.com --reason owner
```

(or set `AGENT_PLATFORM_ADMIN_EMAILS` before start).

## Postgres (Neon)

SQLite is enough for a single local process. Use Neon when the API is public or you want a database that survives the box.

1. Create a project at [console.neon.tech](https://console.neon.tech) (or `npx neonctl@latest projects create --name agent-platform`).
2. Copy the **direct** connection string (host without `-pooler`). The pooled URL is for serverless HTTP clients; sqlx migrations need a real session.
3. Put it in `.env` (never commit it):

```
DATABASE_URL=postgresql://USER:PASSWORD@ep-….aws.neon.tech/neondb?sslmode=require
```

Strip `channel_binding=require` if the console added it — sqlx 0.8 rejects that option. The server also strips it at startup.

4. Start `agent-platformd` (or `agent-platformd migrate`). It applies `migrations/postgres/` (`0001`…`0004_accounts`) on boot.

`TEST_DATABASE_URL` is what `cargo test -p agent-platform-server --test postgres_schema` uses; it creates a scratch schema and drops it. Point it at the same Neon branch only if you accept that extra schema appearing briefly.
