# 8. Web search: build the query, hand it to the browser

Date: 2026-08-15

## Status

**Accepted, then amended the same day** — see
[Amendment: results, behind a key](#amendment-results-behind-a-key). The query
builder and the browser hand-off are unchanged and remain the default. What
changed is the deferral: reading results back is now **adopted, gated on a key
the user supplies**, because the premise the deferral rested on turned out to be
wrong.

The original decision and its reasoning are preserved below unedited, because
the reasoning that produced it is still what governs the no-key path — which is
every install until someone configures a key.

Plan and phases: [`docs/web-search-module-plan.md`](../web-search-module-plan.md).

## Context

The ask had two halves:

1. **Make Google dork usable.** `site:`, `filetype:`, `intitle:`, quoting,
   `-exclusion`, `OR` groups and date bounds are powerful and almost nobody
   writes them. Turn a sentence into the operator string.
2. **Find the right data.** A price table across retailers was the example
   given; the ask is wider than that, and "find a pdf with this title" is the
   same ask in a form the query alone answers. The distinction that ended up
   mattering is **finding** (a document, a discussion, a paper, a page on a
   site — all of it operator work) versus **comparing what came back** (which is
   cheapest, what do these five agree on — which needs the results in hand).

The constraint arrived with the answer to "which provider": **no spend, at least
to start.**

### There is no free programmatic web search

- **Google Programmable Search (CSE) JSON API** — 100 queries/day free, needs a
  Cloud project and an API key plus a search-engine id. Honours the full operator
  set. Paid above the free tier.
- **Brave Search API** — 2000/month free, but the free tier asks for a card.
- **SerpAPI, Serper.dev and the rest** — paid from the first query.
- **Scraping `google.com/search`** — CAPTCHA'd from a datacentre or a residential
  IP alike, against the terms, and breaks on layout changes with no signal. This
  repo's `plan.md` is largely a record of un-breaking things that broke silently;
  adding a dependency on someone else's unversioned HTML is buying more of that.

### But the first half needs none of them

The value in "Google dork is powerful, but not everyone knows how to use it" is
**producing the correct query and showing why**. That is a pure function from a
sentence to a string. Running it is the browser's job, and the user already has a
browser with their own session, their own quota and their own cost of zero.

The app already opens URLs — `shell::open_url`, used by the Providers screen's
"Get API key" button.

## Decision

**The server builds the query. The browser runs it.**

1. One route, `GET /api/v1/search/dork`, taking either `ask=` (a sentence) or
   `q=` (a dork the caller already wrote). It answers the rendered query, the
   parsed parts, a per-operator explanation, and the ready-to-open URL.
2. **The server makes no outbound HTTP calls.** The only thing it talks to is its
   own in-process `/v1` LLM proxy, for the translation.
3. The dork is a **struct** (`DorkQuery`), not a string the model writes. The
   model — when the deterministic rules are not enough — emits the *fields*; Rust
   renders the operators. A model cannot invent an operator that does not exist,
   and the parts are what the dashboard renders as editable chips.
4. **A table of named intent recipes runs first**, and the model only when no
   recipe fires. `document`, `discussion`, `academic`, `docs`, `shopping`, plus
   three modifiers (`onsite`, `recent`, `exclusion`) that compose rather than
   replace. The intent is the knowledge the user is missing, so the recipe's own
   sentence — "Looking for a document, restricted to PDFs, title match" — leads
   the explanation ahead of the per-operator lines. Free and instant, and it
   covers the asks that motivated the module.
5. **No exposure-hunting recipes ship.** `intitle:"index of"`, `filetype:env`
   and the exposed-config dork families are absent from the table. A user typing
   one into `q=` gets it built faithfully, because that is their query and the
   module is a query builder. Shipping a curated table of them is a different
   product — a recon tool — and was not what was asked for. Recorded here rather
   than left implicit, because the table is the obvious place someone later adds
   them without noticing the line being crossed.
6. The route is a **GET under `/api/v1/`**, which is the whole of E.V.'s access:
   `assistant_tools::api_get` is already a prefix-guarded GET-only reader for any
   such route. No new tool for reading, no confirm card in front of a read.
7. **Opening the browser is the one thing that gets a confirm card.** It leaves
   the app, so E.V.'s `web_search` tool parks as a `Pending` and shows the query
   and destination URL verbatim, exactly as `run_command` does.

## Consequences

**Good.**

- Zero cost, zero key, zero quota, nothing to sign up for. Works on the first
  launch after install.
- Zero maintenance surface. No provider HTML to track, no API version to follow,
  no SSRF surface, no robots question, no rate-limit handling — because there is
  no outbound request.
- It teaches. The rendered dork and its explanation are on screen every time, so
  the user who wanted the operators explained ends up able to write them.
- The engine is a URL template, so Google, DuckDuckGo and Bing cost a three-row
  table rather than three integrations.
- Testable end to end with no network in the suite.

**Bad, and accepted.**

- **Results cannot be read back.** No ranking across results, no "which of these
  is cheapest", no summarising five pages, no feeding results into another agent
  step. **Finding** is delivered — the recipe table is exactly that — but
  **comparing** is not, and the price example that opened the ask lands on the
  wrong side of that line.
- **E.V. cannot answer from the web.** It can hand the user a query; it cannot
  read what comes back. Any question of the form "what does the internet say
  about X" stays unanswerable until a provider key exists.
- A browser hand-off is a context switch out of the app. For the query-building
  use case that is correct — the user wanted to search the web — but it means the
  module is a launcher, not a data source.

**The upgrade path, so the deferral is reversible.** When there is a budget:

- `SearchBackend` enum, Google CSE first for operator fidelity. Two arms is not a
  trait; it becomes one at the third.
- `upstream_http::send_with_retry` for the call — it already carries retries,
  rate-limit backoff and `sanitize_url`, which exists precisely because these
  providers take the key as `?key=` and an unsanitized log line is a leaked key.
- The credential goes in `llm_admin::ENV_KEYS`, following `SPEECH_API_KEY` as the
  existing non-chat-provider precedent (see `plan.md`, reuse sweep Pass 2, on why
  those lists are pinned to the provider table rather than generated from it).
- Price extraction, in cost order: snippet regex → `<script
  type="application/ld+json">` schema.org `Product`/`Offer` → og
  `product:price:amount` → model over extracted text, capped. The JSON-LD rung is
  most of the value with no per-site scrapers, because Google rich results push
  nearly every ecommerce page to publish it.
- Fetching result pages is SSRF and takes the `byok.rs` allowlist treatment:
  provider-returned URLs only, never a request parameter or a model's output;
  loopback and RFC1918 refused after resolution; https; 1 MB body cap; 5s.

**Not chosen, and why.**

- **Scraping Google.** Covered above: blocked, against the terms, silently
  fragile.
- **Shipping a key in the binary.** A shared key is one user's abuse away from
  every user's outage, and the key is extractable from the artifact regardless.
- **A headless browser to run the search.** Chromium is a hundreds-of-megabytes
  dependency, detected and blocked much like a scraper, on a desktop app whose
  whole installer is currently two executables.
- **Waiting for a budget before building anything.** The query-building half is
  the half the user asked to be *taught*, it is free, and it is the part a paid
  provider would consume rather than replace: `DorkQuery::render()` is the input
  to a CSE call on the day there is one.

## Amendment: results, behind a key

Date: 2026-08-15, the same day.

**The deferral rested on a factual error.** The Context above says there is no
free programmatic web search, and lists Google CSE as "100 queries/day free,
needs a Cloud project and an API key". Both halves are true; the conclusion drawn
from them was not. **A Cloud project is not a spend** — the CSE free tier needs
no billing account and no card. The constraint given was "no spend", and CSE at
100/day satisfies it. The deferral was therefore reasoning from "needs setup" to
"needs money", which are different things, and the difference is the whole
decision.

Brave was the provider that actually wanted a card. Conflating the two is what
produced the wrong call.

**What is adopted.** A `SearchBackend`, Google CSE first, reading
`SEARCH_API_KEY` and `SEARCH_CX`, and a results surface in the app and to E.V.
CSE is the right first arm for the reason the original decision already gives:
it honours the full operator set, so `DorkQuery::render()` feeds it directly and
part 1 is its input rather than something it replaces.

**What does not change, and this is the point of gating rather than switching:**

- **No key configured is the default and stays the whole product.** The browser
  hand-off is not a fallback for a broken state; it is what the module does out
  of the box, on first launch, with nothing signed up for. Every "Good"
  consequence above still describes that path.
- The server still makes no outbound request until a key exists. Zero
  maintenance surface, zero SSRF surface, zero quota, on an unconfigured install.
- A missing key is **not** an error. It is not a nag, not a banner, and not an
  empty result list — an empty list reads as "nothing matched" and sends the
  user hunting for a better query. Where results would appear, the unconfigured
  install shows the *Open in …* button it shows today.

**Still deferred, and the line has not moved:** fetching result pages for
JSON-LD price extraction. CSE returns titles, snippets and URLs; turning those
into a price table means fetching the pages, which is the SSRF surface and the
per-site maintenance ceiling the original decision priced. Results are
**comparing what a search returned**; extraction is **reading what those pages
say**, and only the first is bought by a key.

**What this costs that the original decision did not pay:** a credential in
`llm_admin::ENV_KEYS` and a field on the Providers screen, a quota that can be
exhausted (100/day is generous for a person and nothing for a loop — so anything
automated calling this needs a cap before it exists), and a second code path
that only some installs exercise. That last one is the real price: the no-key
path is the one every developer sees and the keyed path is the one that breaks
quietly. Both need to be driven, not just tested.
