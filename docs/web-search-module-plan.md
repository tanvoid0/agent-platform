# Web search module — plan

Status: **planned, not started.** Written 2026-08-15, revised the same day after
the cost decision below. Nothing in `desktop/crates/` implements any of this —
`grep -i "search|serper|brave|tavily|duckduckgo"` over the crates hits only
unrelated words (a filter box, `Icon::Search`).

Decision record: [ADR 0008](adr/0008-web-search.md).

## What it is

Natural-language web search with two halves the user named:

1. **Google dork, without knowing Google dork.** The operators (`site:`,
   `filetype:`, `intitle:`, `"…"`, `-`, `OR`, `after:`) are powerful and almost
   nobody writes them. The module takes a sentence and produces the operator
   string — *and shows it*, so the module teaches rather than hides.
2. **Finding the right data.** Price comparison was one example of this, not the
   definition of it — "find a pdf with this title" is the same ask and is
   `intitle:"…" filetype:pdf`, which needs nothing but the query. So most of this
   half is **answered by part 1**, through the recipe table below. Only the
   sub-case that needs *results read back and compared* — a price table across
   retailers — is deferred, because that is the only part a free build cannot do.

Reachable from the dashboard, from E.V. and from chat for the price of one,
because all three already go through the same REST surface.

## The two decisions that shape everything

### 1. No search API. The browser is the result surface.

There is no free programmatic web search. Google CSE is 100 queries/day free
(and needs a Cloud project), Brave's free tier wants a card, and scraping
`google.com/search` from a server is CAPTCHA'd, ToS-barred and breaks silently
on layout changes.

So the module **builds the query and hands it to the user's real browser** —
`shell::open_url` ([shell.rs:698](../desktop/crates/app/src/shell.rs:698)), which
the Providers screen already uses for "Get API key". The user's own session, the
user's own quota, zero cost, zero key, nothing to maintain.

This is not a compromise on the stated use case. "Google dork is powerful but
not everyone knows how to use it" is answered completely by producing the correct
query and showing it. What it does *not* answer is reading results back
programmatically — which is what part 2 needed, and which is what a paid key
later buys.

The server therefore makes **no outbound HTTP calls at all** in this build. The
only thing it talks to is its own `/v1` LLM proxy, which is already in-process.

### 2. Every route is a `GET` under `/api/v1/search`, so E.V. gets it for free.

`desktop/crates/app/src/assistant_tools.rs` already gives the assistant
`api_get(path)` — a prefix-guarded, GET-only reader for *any* `/api/v1/` route.
A GET-with-query-params search route needs **no new assistant tool, no new
dispatch arm, no new confirm card**. The work is one added line in `api_get`'s
description (that string is how the model learns a route exists) and one row in
`SCREENS`.

A `POST` would have cost a new tool, or gone through the `Pending::Write` confirm
card — a confirmation prompt in front of a read, which is the exact pattern
`assistant_tools.rs`'s own doc comment says trains users to click through the
cards that matter.

## Routes

| Route | Answers |
|---|---|
| `GET /api/v1/search/dork` | `{query, url, engine, source, recipes, parts, explanation, chips}` — the translation, the ready-to-open URL, and *why*, so it teaches |
| `GET /api/v1/search` | All of the above **plus** `{configured, results, total_estimate}`. `configured: false` on an install with no key, with every other field still populated |
| `GET/POST/DELETE /api/v1/search/history` | The workspace's search log; `DELETE /{id}` for one row |

`/search/dork` and `/search` resolve the query through the **same**
`resolve_dork_query` and build their common fields through the same
`dork_body` — `/search` is `/dork` plus three keys, not a second
implementation that has to be kept in step.

Editing params on both: `drop=<token>` removes one element, `add_field=` +
`add_value=` adds one. The server builds the operator in both directions, so
**no dork grammar exists outside `search_dork.rs`** — checked by grep, not by
assertion. `chips` is what makes that possible: `{token, label, field}` per
removable element, the token produced by the same code that renders it, so a
client can offer removal without knowing what an operator looks like.

`q` and `ask` are the two ways in:

- **`q=` is a dork, verbatim.** No model call. Parsed into parts, re-rendered,
  URL built. **This is what E.V. uses** — it is already an LLM, so making the
  server call a second one to translate a sentence E.V. could have written as
  operators is a round-trip for nothing.
- **`ask=` is a sentence.** The server translates (rules first, model second).
  This is the dashboard's box, and the user who does not know the syntax.

Params: `engine` (`google` default, `duckduckgo`, `bing` — they are URL
templates, so supporting three costs a three-row table), `site`, `filetype`.

`explanation` is a per-operator line (`site:reddit.com — only pages on
reddit.com`). It is the feature, not decoration: the module's job is to leave the
user able to write the next one themselves.

## Part 1 — the dork

`DorkQuery` is a struct, not a string:

```rust
pub struct DorkQuery {
    terms: String,            // free text
    exact: Vec<String>,       // "…"
    any_of: Vec<String>,      // (a OR b)
    exclude: Vec<String>,     // -x
    sites: Vec<String>,       // site: — OR'd when several
    exclude_sites: Vec<String>,
    filetype: Option<String>,          // `ext:` is accepted on parse, never rendered
    intitle: Vec<String>,
    intext: Vec<String>,               // page body contains
    inurl: Vec<String>,
    related: Vec<String>,              // sites similar to this one
    range: Option<(String, String)>,   // 100..200
    after: Option<String>,             // YYYY-MM-DD
    before: Option<String>,
}
```

**Deliberately never fields**, with the reason at the point someone would reach
for them: `cache:` and `link:` (Google removed both — offering them builds
queries that cannot work), `allintitle:`/`allintext:`/`allinurl:` (repeating the
singular form covers the same ground, and a second spelling is a second thing to
keep in step across all six walks below), and `*` (already works — `terms` is
free text, emitted verbatim).

**Every field is walked in six places** — `render`, `parse`, `explain`,
`drop_part`, `chips`, and `validate` where a whitespace guard applies. Missing
one is silent, which is why the invariant test populates every field and walks
the chips: each one must be droppable, dropping each removes exactly one
element, and the query ends empty but for `terms`.

with `render() -> String`, `parse(&str) -> DorkQuery`, `explain() -> Vec<(String,
String)>` and `url(engine) -> String`.

**Why a struct and not "ask the model for a query string":** the rendered string
is deterministic and testable; the model cannot emit an operator that does not
exist or a `site:` with a space in it; the dashboard renders the parts as
editable chips; and `parse` means a user who types raw dork into the box gets the
same chips back. It is the only pure-logic piece in the module, so it carries the
tests.

Two translators, in this order:

1. **Recipes, no model** (`from_phrases`). Not a bag of patterns — a table of
   named *intents*, because the intent is the knowledge the user is missing.
   Each row: trigger phrases, what it contributes to the struct, and one plain
   sentence describing itself.

   | Recipe | Fires on | Contributes |
   |---|---|---|
   | `document` | pdf, manual, datasheet, whitepaper, ebook, "titled"/"called" | `filetype:pdf` + `intitle:"…"` + `exact` |
   | `discussion` | reviews, opinions, "what do people think", reddit, forum | reddit / HN / stackexchange OR group |
   | `academic` | paper, study, research, journal, citation | `filetype:pdf` + arxiv / `.edu` |
   | `docs` | api docs, documentation, reference | `inurl:docs` OR `site:docs.*` |
   | `shopping` | cheap, cheapest, price, deal, buy | retailer OR group |
   | `onsite` | "on <domain>", "from <domain>" | `site:` |
   | `recent` | "since 2024", "last year", "recent" | `after:` |
   | `exclusion` | "not <x>", "excluding <x>", "without <x>" | `-x` |

   The last three are **modifiers**: they compose with a content recipe rather
   than replacing it, and several content recipes matching merge rather than
   competing. Recipes are additive on the struct, so composition falls out
   instead of being special-cased.

   The fired recipes' sentences lead the `explanation` — "Looking for a document
   — restricted to PDFs, title match" is what actually teaches, ahead of the
   per-operator lines. The response carries `recipes: ["document"]` beside
   `source`, and "no recipe fired" is the signal that gates the model call.

   **Not shipped: exposure-hunting recipes** — `intitle:"index of"`,
   `filetype:env`, exposed-config dorks. A user can still type any of those into
   `q=` and the module builds them faithfully, because that is their query.
   Shipping a curated table of them turns a search helper into a recon tool,
   which is not what was asked for. If recon recipes are ever wanted they are a
   deliberate, separately-named addition.
2. **The model, when the rules produce nothing beyond plain terms.** The server
   calls its own `/v1/chat/completions` — the pattern `coder.rs` already uses,
   including `require_master_key_configured` for the "this server cannot call its
   own proxy" 503. The prompt asks for **the JSON fields above, never a query
   string**; the result deserializes into `DorkQuery` and renders through the
   same `render()`. A model that answers with garbage degrades to rule output
   rather than to a broken query. Free against a local Ollama model, which is
   what this install already runs.

## Part 2 — the dashboard

`Screen::Search`, `desktop/crates/app/src/search.rs` (state + `update`) and
`search_view.rs` (render), per the split the root `CLAUDE.md` requires. Kit
functions only — `ui::page`, `ui::card`, `ui::error_bar` — and the
`.spacing(space::SM)` on the scrollable that `desktop/CLAUDE.md` records as the
thing everyone forgets.

One screen:

- **The box** takes a sentence.
- Under it, the rendered dork **in mono and editable**. This is the teaching
  surface. Editing it switches the run from `ask=` to `q=`.
- **Chips** for the parts, each removable — `site:reddit.com ×`.
- **Explanation lines**, one per operator in play.
- **Open in Google** (`shell::open_url`), with the engine picker beside it.
- **Recent searches**, in-memory. Not a table — see Deferred.

Sidebar entry in `screen.rs`'s NAV table, in the `TOOLS` group beside Coder.
`Icon::Search` already exists ([icon.rs:86](../desktop/crates/app/src/ui/icon.rs:86)).

## Part 3 — E.V. and chat

1. `assistant_tools.rs` — add `/api/v1/search/dork?ask=…` to `api_get`'s
   description string.
2. `assistant_tools.rs` — one row in `SCREENS`:
   `("search", Screen::Search, "web search query builder")`. The existing test
   walks that table, so a half-wired row fails the suite.
3. A new sync tool, `web_search`, and this one is worth its ~20 lines: it
   **parks as a `Pending::Search` confirm card** and, on approval, calls
   `shell::open_url`. Without it E.V.'s only answer is a string the user has to
   copy, which is not "E.V. can access it". Opening a browser leaves the app, so
   it goes through the same gate as `run_command` — the card shows the query and
   the destination URL verbatim.

Chat reaches it through the same REST surface with no change at all.

## Phases

Each phase green on `cd desktop && cargo test` **and** `cargo build` (the
runbook's reason: dev-dependencies unify features back into the lib) before the
next starts.

| # | Scope | Files | Depends on |
|---|---|---|---|
| 1 | `DorkQuery` + `render`/`parse`/`explain`/`url`, the rule translator, the engine table. Pure logic. | `server/src/search_dork.rs` | — |
| 2 | `GET /api/v1/search/dork` (both `q=` and `ask=`), `openapi.json` entry, `lib.rs` wiring | `server/src/search.rs`, `lib.rs`, `openapi.json` | 1 |
| 3 | Model translation for `ask=`, degrading to phase 1's rules | `search_dork.rs` | 1, 2 |
| 4 | Client method | `client/src/lib.rs` | 2 |
| 5 | Desktop screen: state, view, nav wiring | `app/src/search.rs`, `search_view.rs`, `screen.rs`, `main.rs` | 4 |
| 6 | E.V.: description line, `SCREENS` row, `web_search` tool + confirm card | `app/src/assistant_tools.rs`, `assistant.rs`, `assistant_view.rs` | 5 |

Phase 1 carries the tests: `render` round-trips through `parse`; each rule fires
on its phrase and on nothing else; an empty query renders empty rather than to a
bare dangling operator; a `site:` with a space in it cannot be constructed; the
URL is percent-encoded. No network anywhere in the suite, because there is no
network in the module.

`openapi.json` is checked in and served verbatim, and `tests/openapi_drift.rs`
drives every documented operation through the router. **The new route must be
added to that document** or it is undocumented — invisible to the drift test and
absent from Settings → API.

## Deferred, deliberately

- **Reading results back — comparing across results, not finding them.** The
  recipe table covers finding: a document, a discussion, a paper, a page on a
  site. What it cannot do is *rank what came back* — "which of these is
  cheapest", "summarise what these five say". That needs a keyed provider. When
  there is a budget the shape is already decided: a
  `SearchBackend` enum (Google CSE first — it honours the full operator set,
  which is the point of part 1), `upstream_http::send_with_retry` for the call
  (it already has retries, rate-limit backoff, and the `sanitize_url` that exists
  because these providers take the key as `?key=`), and `llm_admin::ENV_KEYS`
  for the credential, following the `SPEECH_API_KEY` precedent for a non-chat
  provider. Free tiers when it comes up: **Google CSE 100/day, no billing
  account needed**; Brave 2000/month but asks for a card.
- **Price extraction.** Follows from the above, not before it. The shape when it
  lands: snippet regex → `<script type="application/ld+json">` schema.org
  `Product`/`Offer` (on nearly every ecommerce page, because Google rich results
  require it — this rung is most of the value with no per-site maintenance) → og
  `product:price:amount` → model over text, capped. No `scraper` dependency; the
  JSON-LD block is a bounded regex and `serde_json` is already here. Fetching
  result pages is SSRF surface and gets the `byok.rs` treatment: provider-returned
  URLs only, no loopback/RFC1918 after resolution, https, 1 MB cap, 5s.
- **Price history / watchlists.** This is what actually makes PriceSpy, and it is
  not what "find me a cheap X" needs. When it lands it is one
  `migrations/000N_search_watch.sql` (watch, observation) and it **reuses
  `workflow_engine`'s interval scheduler** rather than growing a second one.
- ~~**Saved searches in the database.**~~ **Landed 2026-08-15.** In-memory
  recents did not survive a restart, were invisible to E.V. and chat, and were
  per-machine. `search_history` is `migrations/{sqlite,postgres}/0002_…` —
  the first schema change since the Alembic squash, so the first real exercise
  of the forward-only rule.
  - **Workspace-scoped, and that was not optional.** The search routes had no
    tenancy because they stored nothing; storing changes that. `agent-platformd`
    is also the cloud artifact (ADR 0007), so unscoped history shows one
    tenant's searches to another. 404, never 401, on another workspace's row.
  - **`opened` is an INTEGER, not a BOOLEAN.** A `bool` on a `FromRow` struct is
    a latent 500 on the `Any` pool — db.rs documents it and this repo has shipped
    two. It serializes to a wire boolean through `wire::sql_flag`.
  - **Recording is an explicit POST, not a side effect of `GET /search/dork`.**
    That route is hit by every chip removal and operator add, so auto-recording
    would fill history with near-duplicate fragments of a query nobody ran. The
    app posts `opened: false` when a query is built and `opened: true` when it is
    actually opened; the second **promotes the existing row** rather than
    inserting beside it.
- **A provider abstraction.** Two arms of an enum is not a trait. It becomes one
  at the third arm, if there is one.
