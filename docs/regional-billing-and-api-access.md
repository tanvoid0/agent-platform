# Regional billing and API access

Follow this document. It is the locked plan, not a menu of options.

**Product rule:** store apps stay free. One Portal account. One monthly AI subscription. Every allowlisted app shares it. Different countries pay different *amounts* (purchasing-power pricing), not one USD price converted.

**Auth today** (`desktop/crates/server/src/auth.rs`): master key (operator) and `agp_…` workspace tokens (your servers). `X-Agent-Platform-Client` is a label, not a lock. Some `/v1/*` routes authenticate late or not at all. There are no user accounts, entitlements, or Stripe.

---

## 1. Identity

| Layer | Job | Secret? | Who holds it |
|---|---|---|---|
| User | Email. The one billed person. | No | Person |
| Entitlement | `trial` / `paid` / `comp` / `blocked` | No | Server |
| Billing region | Frozen ISO country after first paid invoice | No | Server + Stripe |
| App id | `portal-desktop`, `portal-equalizer`, `android-*` | No — public allowlist | Baked into each binary |
| Device session | Refresh token + short-lived JWT | Yes — on that device | Issued after sign-in |
| Workspace token `agp_…` | Your own services / local admin | Yes | Never ship in a store app |
| Master key | Operator / mint tokens / bind off-loopback | Yes | Server env only |

Store apps ship **only** the public API URL and the public app id. The user signs in. The server issues the session.

---

## 2. User flow

1. Download the free app. Core features work with no account.
2. First AI action → email magic link. Account created. **14-day trial starts. No card. No regional price on screen.** Copy is: “AI Access, billed monthly after trial.”
3. Same login works in every allowlisted app. One entitlement covers all of them.
4. Trial ends → Stripe Checkout. Stripe collects billing country and card. **This is the first time a regional amount appears.** Price = higher of card-issuing country vs billing-address country (see §4–5).
5. Paid → entitlement `paid`. Complimentary accounts skip Checkout (`grant-comp email reason [expires]`).

“Request access” **is** signup. Do not add a ticket queue.

---

## 3. Entitlement states

| State | How they get it | AI |
|---|---|---|
| `trial` | Auto on first signup | Works, capped quota, 14 days |
| `paid` | Stripe subscription current | Works across every allowlisted app |
| `comp` | You grant | Same as paid, no invoice, no region |
| `blocked` | Trial ended, payment failed, or you revoked | Apps stay free; AI returns a subscribe payload |

---

## 4. Regional billing

### How region is decided

| Moment | Source | Shown to user? |
|---|---|---|
| Trial | Nothing | No country, no price |
| Checkout | Stripe Customer billing country **and** card issuing country | Yes — the amount that will actually be charged |
| After first paid invoice | Frozen `billing_region` on the user | Yes — on invoices / Customer Portal |
| IP / ASN / VPN | Server logs only | **Never** |

IP is not a price input and not a UI input. Do not show “you appear to be in…”, detected location, or VPN status.

### Price catalog (v1)

Separate Stripe **Price IDs**. Different amounts. Not FX of one USD price.

| Region key | Countries | Charge currency | Amount (set these) | Stripe Price ID (fill when created) |
|---|---|---|---|---|
| `US` | United States | USD | higher PPP | `price_…` |
| `GB` | United Kingdom | GBP | higher PPP | `price_…` |
| `BD` | Bangladesh | USD at a **lower** amount | lower PPP | `price_…` |
| `ROW` | everywhere else | USD mid-tier | mid PPP | `price_…` |

**Why BD is USD, not BDT, on v1:** keep one Stripe account and one settlement path. Purchasing-power is the *amount*, not the currency glyph. Add a `BDT` price later only if the Stripe account can settle it — same catalog row, new Price ID.

**Tax (v1):** turn on Stripe Tax. UK VAT follows billing country `GB`. Do not build a tax engine. Do not bake VAT into the catalog amount yourself.

**Trial and `comp`:** ignore the catalog. No Price ID is selected.

### Checkout rule

```
region = max_price(catalog[card_country], catalog[billing_address_country])
```

If one side is missing, use the other. If they disagree, charge the **higher** catalog (or refuse Checkout until they match — same outcome for abuse). Example: US-issued Visa + BD address → `US` price.

### Freeze

- First successful paid invoice writes `billing_region` and the Price ID.
- Self-serve cannot change country.
- Admin/support can change it; it takes effect **next** billing cycle (cancel+resubscribe to the new Price ID, or Stripe subscription update).
- VPN during trial steals nothing — trial is free and shows no regional price.

### Add a country later

1. Pick the region key (reuse `ROW` or add `IN`, `DE`, …).
2. Create a new Stripe Price (own amount + currency).
3. Add one row to the catalog table in config.
4. Deploy. Existing frozen subscribers are unchanged until an admin moves them.

---

## 5. VPN and “fake Bangladesh” abuse

**Attack:** someone in the US/UK runs Mullvad, types a BD billing address, hopes for the BD price.

**Why it fails:** price is not IP. A US-issued Visa vs a BD address resolves to the **higher** catalog (`US`/`GB`). They pay the high-price region.

**What we do**

1. Never price from IP or VPN location.
2. Card country vs billing country → higher catalog (or reject until they match).
3. Freeze region on first paid invoice. Country change is support-only, next cycle.
4. Trial shows no regional price. First paid invoice uses card+billing.
5. `comp` skips pricing.
6. Log, do not hard-block: IP country vs billing country, rapid country flips, datacenter/VPN ASN. Fraud review and Radar score only. Do not block travelers or BD users on mobile CGNAT.
7. Do **not** ship “detect VPN and block” as the product. It is leaky, punishes privacy-conscious users, and is bypassable.
8. Later, optional: 3D Secure; Stripe Radar rule “card country US/GB + selected price BD → block.”

**UX lock:** never display IP country, VPN status, or detected location. If you add a public pricing page later, it is a **static table the user picks from**. Checkout still re-prices from card+billing.

---

## 6. Abuse, spam, and hackers

Keep this boring. Real users should not see CAPTCHAs or KYC. The goal is to stop someone from burning your LLM bill or walking in with a leaked key — not to “detect hackers.”

### What they actually try

| Threat | What they want | Why v1 stops it |
|---|---|---|
| Spam / trial farms | Mass free accounts to drain OpenAI/Anthropic | Magic link (email verified), one trial per email, disposable-email blocklist, trial token/request caps, per-user `/v1` rate limit |
| Stolen session | Call `/v1` as that user | Short-lived JWT, refresh revoke, `blocked` entitlement |
| Brute-force / inbox spray | Guess passwords or flood magic links | No passwords. Per-IP limit on magic-link. Same response whether the email exists |
| Scrape `/v1` | Free inference without an account | Entitlement gate on every `/v1` and paid coder route. Unknown `app_id` rejected. Pre-auth `/v1` rate-limited then 401 |
| Leaked master key | Full operator access | Master key only in server env. Process bound to `127.0.0.1` behind TLS. Rotate if it ever leaves the box |
| Reverse-engineered APK | Billing rights | They get a public `app_id` and API URL. That is not an entitlement. They still need a user session |

### V1 — must exist before the public URL

1. **TLS in front** (Caddy or Cloudflare). `agent-platformd` stays on `127.0.0.1`. Master key required off-loopback.
2. **Magic-link signup** (already email-verified) + **one trial per email**.
3. **Hard entitlement gate** on every `/v1/*` and paid coder route: only `trial` / `paid` / `comp`. `blocked` gets a subscribe payload, not a model.
4. **Per-user rate limits** and a **trial token/request cap** so a free account cannot drain LLM spend.
5. **Per-IP rate limits** on magic-link and on `/v1` before a valid session exists.
6. **App-id allowlist** (`portal-desktop`, `portal-equalizer`, `android-*`). Unknown `app_id` → 401.
7. **Token revoke** + account `blocked`.
8. **Usage metering** (requests + tokens) — already in the plan; this is how you see a farm.
9. **API hygiene:** no stack traces to clients. Magic-link always says “if that email exists, we sent a link.”
10. **Cloudflare (or equivalent) in front of the accounts page / magic-link request** — TLS, DDoS, bot fight. Do **not** put Bot Fight on native-app `/v1` if it breaks Tauri/Android clients.
11. **Disposable-email blocklist** on signup (cheap, high spam ROI).

### V1 do not

- CAPTCHA inside native apps on every AI call.
- Phone KYC on day one.
- A custom WAF ruleset you cannot maintain.
- “Detect hackers” as a product feature.

### Later, only if abused

- CAPTCHA on the **web** magic-link form only.
- Stripe Radar / 3DS at checkout (already optional in §5).
- Device-bound refresh tokens, rotate on use.
- Tighten plus-address / disposable farming if one-trial-per-email is bypassed.
- Require a card for trial (auth-only, $0) if LLM drain becomes real.

---

## 7. Before cloud vs after

**Build locally first**

- [ ] User + entitlement + `billing_region` tables (SQLite is fine)
- [ ] User JWT beside master / `agp_`
- [ ] Hard gate on every `/v1/*` and paid coder route
- [ ] Fake states: `trial`, `comp`, `paid`, `blocked`
- [ ] In-memory catalog (US / GB / BD / ROW) and the higher-of-two resolver
- [ ] App-id allowlist header
- [ ] Usage rows (requests + tokens)
- [ ] Per-user `/v1` rate limit + trial token/request cap
- [ ] Per-IP limit on magic-link
- [ ] Disposable-email blocklist
- [ ] Neutral magic-link copy (no user enumeration)
- [ ] Token revoke + `blocked`
- [ ] `grant-comp` CLI
- [ ] portal-desktop: replace pasted `agp_` token with sign-in
- [ ] Checkout mock: pick card country + billing country → show resolved price (no IP)

**Wait for cloud**

- [ ] Caddy or Cloudflare TLS; keep `agent-platformd` on `127.0.0.1`
- [ ] Master key required
- [ ] Cloudflare bot fight on the **accounts / magic-link page only**
- [ ] Real Stripe Prices + Checkout + Customer Portal + webhooks
- [ ] Stripe Tax
- [ ] Hosted accounts page (login, usage, manage billing) — still no IP country
- [ ] Per-user rate limits and revoke (if not already local)
- [ ] Server-side IP/ASN mismatch logs — never in UI
- [ ] Point store builds at the public URL

Postgres waits until a second instance is actually needed. The server is SQLite-only today.

---

## 8. Phased roadmap

### Phase 0 — Local entitlement gate

1. Add `users`, `sessions`, `entitlements` (`trial|paid|comp|blocked`), nullable `billing_region`.
2. Resolve JWT → user. Master and `agp_` stay for operators and your services.
3. Reject `/v1/*` unless entitlement is `trial`, `paid`, or `comp`.
4. Wire portal-desktop login. Stop asking for a workspace token in the store path.

**Done when:** a local user can trial AI, a `blocked` user cannot, and an `agp_` token still works for your own scripts.

### Phase 1 — Accounts and catalog

1. Magic-link signup. 14-day trial. No price, no country on screen.
2. Catalog config + higher-of-two resolver + freeze-on-paid (paid is still faked).
3. App-id allowlist. Usage meter. `grant-comp`.
4. Trial caps + per-user `/v1` limit + per-IP magic-link limit + disposable-email blocklist.
5. Log IP/ASN mismatches. Never return them to the client.

**Done when:** you can flip fake card/billing countries and see the higher price; a second trial on the same email fails; a `blocked` user cannot call `/v1`; UI still never mentions IP.

### Phase 2 — Stripe

1. Create four Prices (US, GB, BD-lower-USD, ROW). Put IDs in config.
2. Checkout: Stripe collects address + card; your server selects the Price ID from the rule in §4.
3. Webhooks set `paid` / `blocked` and freeze `billing_region`.
4. Customer Portal for cancel / update card. Country change is admin-only.
5. Enable Stripe Tax. Optional later: Radar + 3DS.

**Done when:** a test US card + BD address charges the US Price ID.

### Phase 3 — Cloud

1. TLS proxy. Master key required. Rate limits already on.
2. Cloudflare (or Caddy) in front; bot fight on the accounts page only.
3. Hosted accounts + admin page (see §10). Store apps pointed at the public URL.
4. Comp your own email. Ship.

---

## 9. Do not do this

- Put a master key or `agp_` token in an APK, installer, or Tauri binary.
- Price or *display* region from IP / VPN / “you appear to be in X”.
- Convert one USD price with FX and call it regional billing.
- Let users self-serve switch to a cheaper country.
- Manual “request access” forms for every signup.
- VPN-block as the product.
- CAPTCHA on every native-app AI call, phone KYC, or a custom WAF you cannot maintain.
- Per-app prices, usage-based invoices, or social OAuth on v1.
- Replace workspace tokens — they stay for *your* servers.
- Ship admin-only as a Tauri/iced desktop app, or host portal-desktop / Equalizer as the accounts site.
- Resurrect agent-platform’s deleted `web/` + Tauri shell (ADR 0005). “Tauri for web” is not a browser runtime.

---

## 10. Accounts and admin web

There is no `web/` in this repo. The iced desktop app is the only UI; Docker is API-only. portal-desktop is a Svelte SPA that Tauri wraps — that is a store client, not an admin site.

Admin must be a **hosted page** (phone / anywhere). Same Portal login. `admin` is a flag on the owner, not a second product.

v1 on that page: magic-link, own usage + billing, then for admin — account list, entitlement (`trial`/`paid`/`comp`/`blocked`), usage, grant-comp, revoke, Stripe customer id. Not Grafana. `grant-comp` CLI stays for shell access.

---

## First client

portal-desktop already speaks `/v1` and `/api/v1/coder`. Prove login + entitlement there. Equalizer and Android wait until that path is boring. The AI button is the only paywall.
