//! `GET /api/v1/search/dork`, `GET /api/v1/search` and `/api/v1/search/history`
//! — the web search module's routes (ADR 0008, and its amendment "results,
//! behind a key"; `docs/web-search-module-plan.md`).
//! `search_dork.rs` carries the pure dork logic; this file is the wiring
//! around it (query params in, the model called when the rules alone found
//! nothing, a fixed JSON contract out), the results backend the amendment
//! adds, and the history routes below.
//!
//! **`/dork` still makes no outbound HTTP to any search provider.** The only
//! thing it can call is this server's own `/v1/chat/completions`, in-process
//! via [`crate::llm::complete_internal`] — the same self-call `coder_loop.rs`
//! uses for its own LLM steps. What comes back is a ready-to-open URL; the
//! user's browser runs the actual search.
//!
//! **`/search` resolves the query exactly as `/dork` does** ([`resolve_dork_query`]
//! is the one place that happens), **then, only when a key is configured,
//! runs it.** [`SearchBackend::from_env`] is `None` on every install that has
//! not set `SEARCH_API_KEY`/`SEARCH_CX`, which is the default and stays the
//! whole product for that install: `configured: false`, `results: []`, and
//! every field `/dork` would have populated is still populated — never a
//! 503, never an empty list standing in for "not configured". A key that
//! Google rejects or has run out of quota is a different problem from no key
//! at all, and surfaces as a real error rather than `configured: false`.
//!
//! **`/dork` matches `system.rs`, not a tenant route.** It is an
//! operator/read surface with no workspace data behind it — the global
//! `require_token` layer in `lib.rs::router` already gates `/api/v1/*`, so
//! the handler extracts nothing from [`crate::auth::Principal`] and does not
//! invent a scope for a route that has nothing tenant-scoped to check.
//!
//! **`/history` is the one part of this module with tenant data, so it is
//! the one part that reads [`Principal`].** `/dork` builds a query and stores
//! nothing; `/history` is a log of what a workspace searched, and
//! `agent-platformd` is also the cloud artifact (ADR 0007) — an unscoped log
//! would show one tenant's searches to another. Reads and writes are scoped
//! to `principal.workspace_id`, and a row belonging to a different workspace
//! 404s rather than 401s, same contract as [`crate::projects::assert_access`].
//!
//! **History records both *built* and *opened* queries**, told apart by the
//! `opened` flag — not written automatically inside `/dork`, which chip
//! removals and operator adds hit too, and would fill history with
//! near-duplicate fragments. The app POSTs explicitly: `opened: false` when a
//! query is built, `opened: true` when the user runs it. Posting a query
//! already on file with `opened: false` promotes that row instead of
//! inserting a second one — [`create_history`]'s one bit of real logic.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use url::Url;

use crate::auth::Principal;
use crate::db;
use crate::error::{ApiError, PathId};
use crate::search_dork::{recipe_describes, DorkQuery, Engine};
use crate::upstream_http::{sanitize_url, send_with_retry};
use crate::wire::{sql_flag, sql_now, sql_time};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/search/dork", get(search_dork))
        .route("/api/v1/search", get(search))
        .route(
            "/api/v1/search/history",
            get(list_history).post(create_history).delete(clear_history),
        )
        .route("/api/v1/search/history/{history_id}", delete(delete_history))
}

#[derive(Debug, Deserialize)]
struct DorkParams {
    ask: Option<String>,
    q: Option<String>,
    engine: Option<String>,
    site: Option<String>,
    filetype: Option<String>,
    /// Addresses one part to remove from `q`/`ask`'s result before
    /// re-rendering — the client's own chip label (`site:reddit.com`,
    /// `filetype:pdf`, `-membrane`, `intitle:"Foo Bar"`), since it is what
    /// `DorkQuery::render` already produced for that piece. See
    /// `DorkQuery::drop_part`. Unmatched is a no-op, not a 400.
    drop: Option<String>,
    /// Paired with `add_value` — the inverse of `drop`: adds one operator to
    /// the query without the caller ever spelling dork syntax. `add_field` is
    /// a `DorkQuery` field name (see `DorkQuery::add_part`'s match arms);
    /// `add_value` is the raw value. Built server-side by
    /// `DorkQuery::add_part`, which is why this is `Result`, not another
    /// silent no-op like `drop` — a failed *add* is the user's action doing
    /// nothing with no feedback, worse than a named 400.
    add_field: Option<String>,
    add_value: Option<String>,
    /// Only read by [`search`] (`GET /api/v1/search`) — capped at
    /// [`CSE_MAX_RESULTS`] there. `/dork` ignores it, same as any other query
    /// param it does not declare.
    limit: Option<u32>,
}

/// `q` and `ask` are the two ways in — `q` wins when both are given, because
/// it is already an operator string and translating it would be a model
/// round-trip for nothing (`docs/web-search-module-plan.md`'s note on why
/// E.V. always sends `q`, never `ask`).
async fn search_dork(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DorkParams>,
) -> Result<Response, ApiError> {
    let (query, source, recipes, engine) = resolve_dork_query(&state, &params).await?;
    Ok(Json(Value::Object(dork_body(&query, engine, source, &recipes))).into_response())
}

/// `GET /api/v1/search` — the ADR 0008 amendment's results route. Resolves
/// the query exactly as [`search_dork`] does, by calling the same
/// [`resolve_dork_query`], then runs it through [`SearchBackend::from_env`]
/// when one is configured.
///
/// `configured` is what a caller checks before trusting `results` — see this
/// module's doc comment for why an unconfigured install must not just answer
/// an empty list.
async fn search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DorkParams>,
) -> Result<Response, ApiError> {
    let (query, source, recipes, engine) = resolve_dork_query(&state, &params).await?;
    let mut body = dork_body(&query, engine, source, &recipes);

    match SearchBackend::from_env() {
        None => {
            body.insert("configured".to_string(), json!(false));
            body.insert("results".to_string(), json!([]));
            body.insert("total_estimate".to_string(), Value::Null);
        }
        Some(backend) => {
            let limit = params.limit.unwrap_or(CSE_MAX_RESULTS).clamp(1, CSE_MAX_RESULTS);
            let outcome = backend.search(&state, &query.render(), limit).await?;
            body.insert("configured".to_string(), json!(true));
            body.insert("results".to_string(), json!(outcome.results));
            body.insert(
                "total_estimate".to_string(),
                outcome.total_estimate.map(Value::from).unwrap_or(Value::Null),
            );
        }
    }

    Ok(Json(Value::Object(body)).into_response())
}

/// The part of the route both `/dork` and `/search` share: `q`/`ask` in,
/// chip edits and overrides applied, an [`Engine`] resolved. Extracted so
/// `/search` reuses this exactly rather than forking it — a dork typed or
/// translated one way answers identically from either route.
async fn resolve_dork_query(
    state: &AppState,
    params: &DorkParams,
) -> Result<(DorkQuery, &'static str, Vec<&'static str>, Engine), ApiError> {
    let q_raw = params.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ask_raw = params.ask.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let (mut query, source, recipes): (DorkQuery, &'static str, Vec<&'static str>) =
        match (q_raw, ask_raw) {
            (Some(q), _) => (DorkQuery::parse(q), "verbatim", Vec::new()),
            (None, Some(ask)) => {
                let (rules_query, fired) = DorkQuery::from_phrases(ask);
                if fired.is_empty() {
                    // No recipe found anything beyond plain terms — the one
                    // case worth a model round-trip. Any failure at all (no
                    // master key, no model reachable, bad JSON, a timeout)
                    // falls straight back to the rule output computed above;
                    // this route must never 500 because a model misbehaved.
                    match translate_with_model(state, ask).await {
                        Some(model_query) => (model_query, "model", Vec::new()),
                        None => (rules_query, "rules", fired),
                    }
                } else {
                    (rules_query, "rules", fired)
                }
            }
            (None, None) => {
                return Err(ApiError::bad_request(
                    "Provide either `ask` (a sentence) or `q` (a dork already written).",
                ));
            }
        };

    // Chip removal: drop the one piece the caller named before anything else
    // runs — an unmatched token is a no-op, per `DorkQuery::drop_part`.
    if let Some(token) = params.drop.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query.drop_part(token);
    }

    // The inverse of `drop`: add one operator, built server-side. Unlike
    // `drop`, a bad `add_field`/`add_value` is a 400 naming the problem —
    // see `DorkParams::add_field`'s doc comment for why.
    if let Some(field) = params.add_field.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let value = params.add_value.as_deref().unwrap_or("");
        query.add_part(field, value).map_err(ApiError::bad_request)?;
    }

    // Query-param overrides apply last, regardless of source — they are the
    // caller correcting or narrowing what the translation produced.
    if let Some(site) = params.site.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let _ = query.add_site(site);
    }
    if let Some(ft) = params.filetype.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query.filetype = Some(ft.to_string());
    }

    let engine = params.engine.as_deref().and_then(Engine::parse).unwrap_or_default();
    Ok((query, source, recipes, engine))
}

/// The `query`/`url`/`engine`/`source`/`recipes`/`parts`/`explanation`/`chips`
/// fields both `/dork` and `/search` answer — `/search`'s response is exactly
/// this plus `configured`/`results`/`total_estimate` (ADR 0008's amendment).
fn dork_body(
    query: &DorkQuery,
    engine: Engine,
    source: &'static str,
    recipes: &[&'static str],
) -> Map<String, Value> {
    // Each fired recipe's own sentence comes first — "Looking for a
    // document — restricted to PDFs…" is the line that actually teaches,
    // ahead of the per-operator detail `explain()` gives. `kind` tells the
    // two row shapes apart: a recipe row's `label` is a recipe name
    // ("document"), not an operator, and folding it into an `operator` field
    // is the exact mislabelling class this repo already shipped once for
    // tool rows (see plan.md, "Transcript labels lied").
    let mut explanation: Vec<Value> = recipes
        .iter()
        .filter_map(|name| recipe_describes(name).map(|meaning| json!({ "kind": "recipe", "label": name, "meaning": meaning })))
        .collect();
    explanation.extend(
        query
            .explain()
            .into_iter()
            .map(|(operator, meaning)| json!({ "kind": "operator", "label": operator, "meaning": meaning })),
    );

    let mut out = Map::new();
    out.insert("query".to_string(), json!(query.render()));
    out.insert("url".to_string(), json!(query.url(engine)));
    out.insert("engine".to_string(), json!(engine.as_str()));
    out.insert("source".to_string(), json!(source));
    out.insert("recipes".to_string(), json!(recipes));
    out.insert("parts".to_string(), serde_json::to_value(query).unwrap_or_else(|_| json!({})));
    out.insert("explanation".to_string(), json!(explanation));
    out.insert("chips".to_string(), json!(query.chips()));
    out
}

// ---------------------------------------------------------------------------
// Results backend — ADR 0008's amendment, "results, behind a key"
// ---------------------------------------------------------------------------

/// Google's Custom Search JSON API returns at most 10 results per call.
/// `/search`'s `limit` is capped at this, never paginated to fake a bigger
/// number — `total_estimate` is what tells the caller more exists.
const CSE_MAX_RESULTS: u32 = 10;

/// Reads results back for a query [`DorkQuery::render`] produced, gated on a
/// user-supplied key (the ADR 0008 amendment). **Two arms is not a trait and
/// one arm is not an enum** — but the ADR already names Brave as the second
/// arm and the shape is known, so this is an enum now rather than a struct
/// that grows into one later. Not a trait, not a registry, not a plugin
/// point: there is one caller and it is this file.
enum SearchBackend {
    GoogleCse { key: String, cx: String },
}

impl SearchBackend {
    /// `None` when unconfigured — the default, and every install until an
    /// operator sets both `SEARCH_API_KEY` and `SEARCH_CX`.
    fn from_env() -> Option<Self> {
        Self::from_parts(
            crate::llm_config::from_env_or_dotenv("SEARCH_API_KEY"),
            crate::llm_config::from_env_or_dotenv("SEARCH_CX"),
        )
    }

    /// Split from [`Self::from_env`] so "both required" is testable without
    /// mutating process environment. **Both** are required: a key without a
    /// cx is unconfigured, not half-configured, and must report itself as
    /// unconfigured rather than failing the first time it is used.
    fn from_parts(key: String, cx: String) -> Option<Self> {
        let key = key.trim().to_string();
        let cx = cx.trim().to_string();
        if key.is_empty() || cx.is_empty() {
            return None;
        }
        Some(SearchBackend::GoogleCse { key, cx })
    }

    /// ponytail: CSE's free tier is 100 queries/day — generous for a person,
    /// nothing for a loop, and nothing in this codebase stops a workflow step
    /// or a retrying agent from burning the whole day's budget in seconds.
    /// A quota manager is the upgrade path the day something automated calls
    /// this route; it is not built here.
    async fn search(&self, state: &AppState, query: &str, limit: u32) -> Result<CseOutcome, ApiError> {
        match self {
            SearchBackend::GoogleCse { key, cx } => {
                google_cse_search(state, key, cx, query, limit).await
            }
        }
    }
}

struct CseOutcome {
    results: Vec<Value>,
    total_estimate: Option<i64>,
}

fn cse_url(key: &str, cx: &str, query: &str, num: u32) -> Url {
    let mut url =
        Url::parse("https://www.googleapis.com/customsearch/v1").expect("static CSE base URL parses");
    url.query_pairs_mut()
        .append_pair("key", key)
        .append_pair("cx", cx)
        .append_pair("q", query)
        .append_pair("num", &num.to_string());
    url
}

/// The one outbound call this module makes, and only once a key is
/// configured. Goes through [`send_with_retry`] for its retry policy and,
/// critically, [`sanitize_url`] — Google takes the key as `?key=`, the same
/// shape as the Gemini leak `sanitize_url` exists for, so the raw URL must
/// never reach a log line.
async fn google_cse_search(
    state: &AppState,
    key: &str,
    cx: &str,
    query: &str,
    limit: u32,
) -> Result<CseOutcome, ApiError> {
    let url = cse_url(key, cx, query, limit.clamp(1, CSE_MAX_RESULTS));
    let url_str = url.to_string();
    // No rate-limit retries: CSE's limit here is a 100/day quota, not a
    // per-second burst — retrying it spends more of the quota it just told us
    // is gone, rather than recovering from a transient blip.
    let response = send_with_retry("search_cse", false, || state.http.get(url_str.clone())).await?;

    if !response.status.is_success() {
        // A rejected or quota-exhausted key is a real error, distinct from
        // `configured: false` — "your key stopped working" and "you have no
        // key" are different problems with different fixes, and collapsing
        // them is how someone spends an hour on the wrong one.
        let message = response
            .json()
            .as_ref()
            .and_then(|b| b.get("error"))
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Search provider returned {}.", response.status));
        logd!("search cse rejected status={} url={}", response.status, sanitize_url(&url_str));
        return Err(ApiError::coded(response.status, "search_provider_error", message));
    }

    let body = response.json().unwrap_or_else(|| json!({}));
    Ok(parse_cse_body(&body))
}

/// `items[].{title,link,displayLink,snippet}` — the wire shape this route
/// answers — plus `searchInformation.totalResults` (a string in CSE's own
/// JSON) parsed to a number. Pure and network-free, which is what lets the
/// test below use a canned response body instead of a live call.
fn parse_cse_body(body: &Value) -> CseOutcome {
    let results: Vec<Value> = body
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    json!({
                        "title": item.get("title").and_then(Value::as_str).unwrap_or_default(),
                        "url": item.get("link").and_then(Value::as_str).unwrap_or_default(),
                        "domain": item.get("displayLink").and_then(Value::as_str).unwrap_or_default(),
                        "snippet": item.get("snippet").and_then(Value::as_str).unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let total_estimate = body
        .get("searchInformation")
        .and_then(|si| si.get("totalResults"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok());
    CseOutcome { results, total_estimate }
}

/// Phase 3: the model is asked for `DorkQuery`'s *fields*, never a query
/// string — so it cannot emit an operator that does not exist or a `site:`
/// with a space in it, the same guarantee [`DorkQuery::render`] gives the
/// rule path. `None` on any failure; the caller already has the rule output
/// to fall back to.
async fn translate_with_model(state: &AppState, ask: &str) -> Option<DorkQuery> {
    // Mirrors `coder.rs::require_master_key_configured` — this server has no
    // master key, so it cannot call its own `/v1` at all. Not
    // `Principal::require_master_key`: that one is the tenancy check, this
    // one is "there is no model to call" (`plan.md` Pass 4 on the naming).
    state.master_key.as_ref()?;

    let mut payload = Map::new();
    payload.insert(
        "messages".into(),
        json!([
            { "role": "system", "content": TRANSLATE_SYSTEM_PROMPT },
            { "role": "user", "content": ask },
        ]),
    );
    payload.insert("max_tokens".into(), json!(400));

    let data = crate::llm::complete_internal(state, payload, crate::resources::Priority::Interactive).await.ok()?;
    let content = data.get("choices")?.as_array()?.first()?.get("message")?.get("content")?.as_str()?;
    let candidate: DorkQuery = serde_json::from_str(extract_json_object(content)?).ok()?;
    candidate.validate().ok()?;
    Some(candidate)
}

const TRANSLATE_SYSTEM_PROMPT: &str = "You translate a natural-language web search request into \
    the JSON fields of a Google dork query. Respond with ONLY a JSON object — no markdown fences, \
    no commentary — using any of these optional fields: terms (string), exact (array of exact-match \
    phrases), any_of (array of alternative terms), exclude (array of terms to exclude), sites (array \
    of bare domains, no protocol, no path, no whitespace), exclude_sites (array of bare domains), \
    filetype (a bare extension like \"pdf\", or omit), intitle (array of phrases the page title must \
    contain), inurl (array of phrases the page address must contain), after (date as YYYY-MM-DD, or \
    omit), before (date as YYYY-MM-DD, or omit). Omit any field that does not apply. Never invent an \
    operator outside this field list.";

/// The outermost `{…}` span in a model's reply — `find('{')` paired with
/// `rfind('}')`, not the first balanced object. Permissive against a model
/// that wraps its JSON in a code fence or a sentence despite being told not
/// to: prose before the first brace and after the last both fall away, and
/// prose *between* two JSON-shaped chunks still yields one combined span
/// rather than only the first chunk — the whole point being that
/// prose-then-JSON-then-prose degrades to the whole object, not a truncation
/// of it.
fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    (end >= start).then(|| &s[start..=end])
}

// ---------------------------------------------------------------------------
// GET/POST/DELETE /api/v1/search/history
// ---------------------------------------------------------------------------

#[derive(Debug, FromRow, Serialize)]
struct SearchHistoryOut {
    id: i64,
    workspace_id: Option<i64>,
    query: String,
    engine: String,
    source: String,
    /// `INTEGER` on both backends, not `BOOLEAN` — see the migration's
    /// comment and `db.rs`'s note on what the `Any` driver will decode.
    #[serde(serialize_with = "sql_flag")]
    opened: i64,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
}

const HISTORY_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(workspace_id AS BIGINT) AS workspace_id, query, engine, source, opened, \
     CAST(created_at AS TEXT) AS created_at";

/// A list request is capped, like every other list route here — an
/// unbounded caller-supplied `limit` is a memory hazard, not a feature.
const HISTORY_LIMIT_CAP: i64 = 200;

fn default_history_limit() -> i64 {
    50
}

async fn load_history(state: &AppState, id: i64) -> Result<SearchHistoryOut, ApiError> {
    sqlx::query_as(&db::sql(
        &format!("SELECT {HISTORY_COLUMNS} FROM search_history WHERE id = ?"),
        state.backend,
    ))
    .bind(id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Search history entry not found"))
}

/// `assert_access` + `require_one`, mirroring [`crate::projects::assert_access`]:
/// 404 — never 401 — is the tenancy contract, so a workspace token asking
/// about another tenant's row must not learn that it exists.
#[derive(FromRow)]
struct HistoryOwner {
    workspace_id: Option<i64>,
}

async fn assert_history_access(
    state: &AppState,
    principal: &Principal,
    history_id: i64,
) -> Result<(), ApiError> {
    let row: Option<HistoryOwner> = sqlx::query_as(&db::sql(
        "SELECT CAST(workspace_id AS BIGINT) AS workspace_id FROM search_history WHERE id = ?",
        state.backend,
    ))
    .bind(history_id)
    .fetch_optional(&state.any)
    .await?;

    let Some(row) = row else {
        return Err(ApiError::not_found("Not found"));
    };
    // The master key is unrestricted, same as everywhere else it appears —
    // it sees and manages every workspace's history, which is why the single-
    // row delete and the clear-all route below both go unfiltered for it.
    match principal.workspace_id {
        None => Ok(()),
        Some(ws) if Some(ws) == row.workspace_id => Ok(()),
        Some(_) => Err(ApiError::not_found("Not found")),
    }
}

#[derive(Debug, Deserialize)]
struct ListHistoryParams {
    #[serde(default = "default_history_limit")]
    limit: i64,
    #[serde(default)]
    opened_only: bool,
}

async fn list_history(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ListHistoryParams>,
) -> Result<Response, ApiError> {
    let mut sql = format!("SELECT {HISTORY_COLUMNS} FROM search_history");
    let mut wheres: Vec<&str> = Vec::new();
    if principal.workspace_id.is_some() {
        wheres.push("workspace_id = ?");
    }
    if q.opened_only {
        wheres.push("opened != 0");
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");

    // Owned, not borrowed: this string is computed, and a `Cow::Borrowed`
    // over a temporary that drops at the end of this statement is exactly
    // the bug `db.rs`'s doc comment warns a computed query invites — see
    // `processes::list_processes` for the same pattern.
    let sql = db::sql(&sql, state.backend).into_owned();
    let mut query = sqlx::query_as::<_, SearchHistoryOut>(&sql);
    if let Some(ws) = principal.workspace_id {
        query = query.bind(ws);
    }
    let rows = query.bind(q.limit.clamp(1, HISTORY_LIMIT_CAP)).fetch_all(&state.any).await?;

    Ok(Json(json!({ "history": rows })).into_response())
}

#[derive(Debug, Deserialize)]
struct HistoryCreate {
    query: Option<String>,
    engine: Option<String>,
    source: Option<String>,
    #[serde(default)]
    opened: bool,
}

/// Records one dork the caller either built (`opened: false`) or ran
/// (`opened: true`). Not called automatically from `/dork` — see this
/// module's doc comment for why — so this is the only write path.
///
/// **The one bit of real logic**: posting a query already on file for this
/// workspace with `opened: false` and now `opened: true` promotes that row
/// (flips the flag, bumps `created_at`) instead of inserting a near-duplicate.
/// A first-time `opened: true` with no matching row, and every `opened: false`
/// post, just inserts.
async fn create_history(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let req: HistoryCreate = crate::wire::parse_body_typed(&body)?;
    let query = req
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("`query` is required."))?
        .to_string();
    let engine = req
        .engine
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("`engine` is required."))?
        .to_string();
    let source = req
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("`source` is required."))?
        .to_string();

    let now = sql_now();

    if req.opened {
        // Scoped to the caller's own workspace (or, for the master key, to
        // the rows it owns itself — `workspace_id IS NULL`) so promotion can
        // never flip a flag on a row that belongs to somebody else.
        let existing: Option<i64> = match principal.workspace_id {
            Some(ws) => sqlx::query_scalar(&db::sql(
                "SELECT CAST(id AS BIGINT) FROM search_history \
                 WHERE workspace_id = ? AND query = ? AND opened = 0 \
                 ORDER BY id DESC LIMIT 1",
                state.backend,
            ))
            .bind(ws)
            .bind(&query)
            .fetch_optional(&state.any)
            .await?,
            None => sqlx::query_scalar(&db::sql(
                "SELECT CAST(id AS BIGINT) FROM search_history \
                 WHERE workspace_id IS NULL AND query = ? AND opened = 0 \
                 ORDER BY id DESC LIMIT 1",
                state.backend,
            ))
            .bind(&query)
            .fetch_optional(&state.any)
            .await?,
        };

        if let Some(id) = existing {
            sqlx::query(&db::sql(
                "UPDATE search_history SET opened = 1, created_at = ? WHERE id = ?",
                state.backend,
            ))
            .bind(&now)
            .bind(id)
            .execute(&state.any)
            .await?;
            return Ok((StatusCode::OK, Json(load_history(&state, id).await?)).into_response());
        }
    }

    let id: i64 = sqlx::query_scalar(&db::sql(
        "INSERT INTO search_history (workspace_id, query, engine, source, opened, created_at) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .bind(principal.workspace_id)
    .bind(&query)
    .bind(&engine)
    .bind(&source)
    .bind(i64::from(req.opened))
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    Ok((StatusCode::CREATED, Json(load_history(&state, id).await?)).into_response())
}

async fn delete_history(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(history_id): PathId<i64>,
) -> Result<Response, ApiError> {
    assert_history_access(&state, &principal, history_id).await?;
    sqlx::query(&db::sql("DELETE FROM search_history WHERE id = ?", state.backend))
        .bind(history_id)
        .execute(&state.any)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Clears every row this caller can see — the master key's own rows are
/// unfiltered here too, matching [`assert_history_access`]'s unrestricted
/// case for the single-row delete above.
async fn clear_history(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    match principal.workspace_id {
        Some(ws) => {
            sqlx::query(&db::sql("DELETE FROM search_history WHERE workspace_id = ?", state.backend))
                .bind(ws)
                .execute(&state.any)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM search_history").execute(&state.any).await?;
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_object_strips_surrounding_prose_and_fences() {
        assert_eq!(extract_json_object(r#"{"terms":"a"}"#), Some(r#"{"terms":"a"}"#));
        assert_eq!(
            extract_json_object("```json\n{\"terms\":\"a\"}\n```"),
            Some("{\"terms\":\"a\"}")
        );
        assert_eq!(extract_json_object("sure, here you go: {\"terms\":\"a\"} thanks"), Some("{\"terms\":\"a\"}"));
        assert_eq!(extract_json_object("no json here"), None);
    }

    // -----------------------------------------------------------------------
    // SearchBackend — configured / unconfigured
    // -----------------------------------------------------------------------

    #[test]
    fn cse_backend_needs_both_key_and_cx() {
        assert!(SearchBackend::from_parts(String::new(), String::new()).is_none());
        assert!(SearchBackend::from_parts("key".into(), String::new()).is_none());
        assert!(SearchBackend::from_parts(String::new(), "cx".into()).is_none());
        assert!(SearchBackend::from_parts("  ".into(), "cx".into()).is_none(), "whitespace-only is empty");
        assert!(matches!(
            SearchBackend::from_parts("key".into(), "cx".into()),
            Some(SearchBackend::GoogleCse { .. })
        ));
    }

    /// Guards the two tests below, which mutate `SEARCH_API_KEY`/`SEARCH_CX`
    /// process environment — every other test in this binary that also
    /// touches process env (see `llm_config.rs`'s `ENV_LOCK`) uses different
    /// variable names, so this only has to serialize against itself.
    static SEARCH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn dork_params(q: &str) -> DorkParams {
        DorkParams {
            ask: None,
            q: Some(q.to_string()),
            engine: None,
            site: None,
            filetype: None,
            drop: None,
            add_field: None,
            add_value: None,
            limit: None,
        }
    }

    async fn search_json(state: Arc<AppState>, params: DorkParams) -> Value {
        let response = search(State(state), Query(params)).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// The path that matters most (ADR 0008's amendment): no key configured
    /// is the default, not an error, and not an empty `results` list standing
    /// in for "not configured" — `configured: false` plus every field `/dork`
    /// would have populated, still populated.
    #[tokio::test]
    async fn unconfigured_search_still_answers_query_url_and_explanation() {
        let _guard = SEARCH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("SEARCH_API_KEY");
        std::env::remove_var("SEARCH_CX");

        let db_path = std::env::temp_dir()
            .join(format!("agp-search-unconfigured-{}.db", std::process::id()));
        let state = Arc::new(AppState::new(&db_path, None));

        let body = search_json(state, dork_params("cheap mechanical keyboard")).await;

        assert_eq!(body["configured"], json!(false));
        assert_eq!(body["results"], json!([]));
        assert_eq!(body["total_estimate"], Value::Null);
        // Everything /dork would have populated is still here — this is the
        // whole point of gating rather than switching (ADR 0008's amendment).
        assert_eq!(body["query"], json!("cheap mechanical keyboard"));
        assert!(body["url"].as_str().unwrap().starts_with("https://www.google.com/search?q="));
        assert_eq!(body["engine"], json!("google"));
        assert_eq!(body["source"], json!("verbatim"));
        assert!(body["parts"].is_object());
        assert!(body["explanation"].is_array());
        assert!(body["chips"].is_array());

        let _ = std::fs::remove_file(&db_path);
    }

    /// A key with no `cx` is unconfigured, not half-configured — same
    /// response shape as no key at all, never an error.
    #[tokio::test]
    async fn a_key_without_a_cx_is_unconfigured_not_an_error() {
        let _guard = SEARCH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("SEARCH_API_KEY", "sk-test-only");
        std::env::remove_var("SEARCH_CX");

        let db_path =
            std::env::temp_dir().join(format!("agp-search-nocx-{}.db", std::process::id()));
        let state = Arc::new(AppState::new(&db_path, None));

        let body = search_json(state, dork_params("keyboard")).await;
        assert_eq!(body["configured"], json!(false));
        assert_eq!(body["results"], json!([]));

        std::env::remove_var("SEARCH_API_KEY");
        let _ = std::fs::remove_file(&db_path);
    }

    // -----------------------------------------------------------------------
    // The CSE call: URL, logging safety, response parsing — all network-free
    // -----------------------------------------------------------------------

    #[test]
    fn cse_url_key_is_redacted_by_sanitize_url_but_cx_is_not() {
        let url = cse_url("sk-secret", "01234:abc", "cheap keyboards", 5);
        let safe = sanitize_url(url.as_str());
        assert!(!safe.contains("sk-secret"), "the key must never reach a log line: {safe}");
        // cx is not a credential (see `SENSITIVE_ENV_KEYS`'s doc comment), so
        // sanitize_url leaves it readable — this is the site `google_cse_search`
        // itself logs through, so proving it here proves the real call site.
        assert!(safe.contains("cx="), "cx should stay readable: {safe}");
        assert!(safe.contains("key=***"), "got: {safe}");
    }

    #[test]
    fn cse_body_parses_into_the_wire_shape_with_no_network() {
        let canned = json!({
            "searchInformation": { "totalResults": "12400" },
            "items": [
                {
                    "title": "Mechanical keyboards under $100",
                    "link": "https://example.com/keyboards",
                    "displayLink": "example.com",
                    "snippet": "A roundup of budget mechanical keyboards."
                },
                {
                    "title": "Keyboard discussion",
                    "link": "https://reddit.com/r/keyboards",
                    "displayLink": "reddit.com",
                    "snippet": "Thread."
                }
            ]
        });
        let outcome = parse_cse_body(&canned);
        assert_eq!(outcome.total_estimate, Some(12400));
        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[0]["title"], json!("Mechanical keyboards under $100"));
        assert_eq!(outcome.results[0]["url"], json!("https://example.com/keyboards"));
        assert_eq!(outcome.results[0]["domain"], json!("example.com"));
        assert_eq!(outcome.results[0]["snippet"], json!("A roundup of budget mechanical keyboards."));
    }

    #[test]
    fn cse_body_with_no_items_parses_to_an_empty_list_not_an_error() {
        let canned = json!({ "searchInformation": { "totalResults": "0" } });
        let outcome = parse_cse_body(&canned);
        assert!(outcome.results.is_empty());
        assert_eq!(outcome.total_estimate, Some(0));
    }
}
