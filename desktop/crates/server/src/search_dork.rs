//! `DorkQuery` — the pure logic behind the web search module (ADR 0008,
//! `docs/web-search-module-plan.md`).
//!
//! A Google dork string (`site:`, `filetype:`, `intitle:`, `"…"`, `-`, `OR`,
//! `after:`/`before:`) is powerful and almost nobody writes it by hand. This
//! module is the translator, in both directions:
//!
//! - [`DorkQuery::render`] turns the struct into the operator string, and
//!   [`DorkQuery::parse`] turns a string back into the struct — so a user who
//!   types raw dork gets the same editable parts back that a translated
//!   sentence would have produced.
//! - [`DorkQuery::from_phrases`] is the free, offline, first-pass translator.
//!   It is not ten ad-hoc patterns: it is a table of named **intent
//!   recipes** (document, discussion, academic, docs, shopping — plus three
//!   modifiers that compose with any of them: onsite, recent, exclusion).
//!   Recipes are additive on the struct, so more than one firing on the same
//!   ask is normal, not a conflict — "cheap pdf manual on arxiv.org" is
//!   `document` and `shopping` and `onsite` all at once. `search.rs`'s route
//!   only reaches for the model when **no recipe fired**.
//! - [`DorkQuery::explain`] is the teaching surface: one plain-English line
//!   per operator actually in play. `search.rs` puts each fired recipe's own
//!   line ahead of these — see [`recipe_describes`].
//!
//! **A struct, not a string the model writes.** The model — in `search.rs`'s
//! phase 3 — emits the *fields*, and this module renders the operators. That
//! is what makes a `site:` with a space in it impossible to ship: it has to
//! come in through [`DorkQuery::add_site`] or [`DorkQuery::validate`], both of
//! which reject it, rather than through a model hallucinating operator syntax
//! directly.
//!
//! **What is deliberately not a recipe.** No `intitle:"index of"`, no
//! exposed-config or directory-listing dorks, no credential-hunting shapes.
//! Typing one into `q=` still works — that is the caller's own query, parsed
//! faithfully — but this table does not curate a way to arrive at one from a
//! sentence. A search helper is not a recon tool.
//!
//! **What is deliberately not a field, ever, not just a recipe** — reach for
//! one of these and stop, the answer is already here:
//! - `cache:` — Google removed it in 2024. A field for it would build queries
//!   that cannot work.
//! - `link:` — deprecated by Google years ago; returns nothing useful.
//! - `allintitle:`/`allintext:`/`allinurl:` — repeating `intitle:`/`intext:`/
//!   `inurl:` covers the same ground and composes with everything else this
//!   struct already has. A second spelling of the same operator is a second
//!   thing to keep in sync across all six of `render`/`parse`/`explain`/
//!   `drop_part`/`chips`/`validate`, for zero new capability.
//! - `*` wildcard — already works. `terms` is free text and `render` emits it
//!   verbatim, so a literal `*` a caller types just passes through.
//!
//! No network here, and nothing model-related — that lives in `search.rs`.
//! This file carries the module's tests, being its only pure logic.

use std::sync::LazyLock;

use chrono::Datelike;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// A parsed/composed Google dork. Every field is a piece the dashboard can
/// render as an editable chip — see `docs/web-search-module-plan.md`'s Part 2.
///
/// `#[serde(default)]` on every field so a model's JSON (phase 3) that omits a
/// field deserializes to "not present" rather than failing the whole request;
/// a malformed model response should degrade to the rule output, not 500.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DorkQuery {
    #[serde(default)]
    pub terms: String,
    #[serde(default)]
    pub exact: Vec<String>,
    #[serde(default)]
    pub any_of: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub exclude_sites: Vec<String>,
    /// `related:` — "sites similar to this one". A bare domain, same shape as
    /// `sites`/`exclude_sites`, so it gets the same whitespace guard
    /// ([`Self::add_related`]) and sits beside them in the field order below.
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub filetype: Option<String>,
    #[serde(default)]
    pub intitle: Vec<String>,
    /// `intext:` — "the page body must contain this". Grouped next to
    /// `intitle`/`inurl` below since it is the same family of operator.
    #[serde(default)]
    pub intext: Vec<String>,
    #[serde(default)]
    pub inurl: Vec<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
    /// Google's `lo..hi` numeric range operator. A tuple, not a struct — two
    /// strings (kept as strings, not parsed numbers, same as `after`/`before`
    /// keeping dates as strings: this module never evaluates the value, only
    /// renders it back).
    #[serde(default)]
    pub range: Option<(String, String)>,
}

/// One removable piece of a [`DorkQuery`], as [`DorkQuery::chips`] emits it —
/// the wire shape a dashboard renders as an "×" badge. `token` is handed back
/// verbatim as `drop=` on the next request and is matched byte-for-byte by
/// [`DorkQuery::drop_part`]; `field` names the `DorkQuery` field the element
/// came from, so a client can pick a chip's tone without parsing dork syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DorkChip {
    pub token: String,
    pub label: String,
    pub field: &'static str,
}

impl DorkQuery {
    /// The only supported way to add a `site:` value from a source that is not
    /// already guaranteed operator-safe (a query-string override, a model's
    /// JSON). A dork token cannot contain whitespace — `site:my site.com` is
    /// two tokens, not one operator — so this is where that gets caught rather
    /// than in [`Self::render`], which must never try to repair bad data.
    pub fn add_site(&mut self, site: &str) -> Result<(), String> {
        let site = site.trim();
        if site.is_empty() {
            return Ok(());
        }
        if site.chars().any(char::is_whitespace) {
            return Err(format!("site {site:?} contains whitespace"));
        }
        self.sites.push(site.to_string());
        Ok(())
    }

    /// Same guard, for `-site:`.
    pub fn add_exclude_site(&mut self, site: &str) -> Result<(), String> {
        let site = site.trim();
        if site.is_empty() {
            return Ok(());
        }
        if site.chars().any(char::is_whitespace) {
            return Err(format!("site {site:?} contains whitespace"));
        }
        self.exclude_sites.push(site.to_string());
        Ok(())
    }

    /// Same guard, for `related:`.
    pub fn add_related(&mut self, site: &str) -> Result<(), String> {
        let site = site.trim();
        if site.is_empty() {
            return Ok(());
        }
        if site.chars().any(char::is_whitespace) {
            return Err(format!("site {site:?} contains whitespace"));
        }
        self.related.push(site.to_string());
        Ok(())
    }

    /// Guards a struct that arrived by a path other than the constructors
    /// above — namely `search.rs`'s phase 3, which deserializes a model's JSON
    /// straight into the public fields. Any failure here means the model's
    /// answer is discarded and the route falls back to rule output.
    pub fn validate(&self) -> Result<(), String> {
        for site in self.sites.iter().chain(self.exclude_sites.iter()).chain(self.related.iter()) {
            if site.trim().is_empty() || site.chars().any(char::is_whitespace) {
                return Err(format!("invalid site {site:?}"));
            }
        }
        Ok(())
    }

    /// The inverse of [`Self::drop_part`] — adds one element to the
    /// `DorkParts` field named `field`, built server-side so no dork grammar
    /// exists outside this module (`search.rs`'s `add_field`/`add_value`
    /// route params are the only caller). Unlike `drop_part`, an unmatched
    /// or invalid request is `Err`, not a silent no-op: a failed *drop* still
    /// leaves the query as the user last saw it, but a failed *add* would
    /// leave the user's action doing nothing with no feedback, which is
    /// worse than a named error.
    pub fn add_part(&mut self, field: &str, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{field} needs a value"));
        }
        match field {
            "sites" => self.add_site(value),
            "exclude_sites" => self.add_exclude_site(value),
            "related" => self.add_related(value),
            "exclude" => {
                self.exclude.push(value.to_string());
                Ok(())
            }
            "exact" => {
                self.exact.push(value.to_string());
                Ok(())
            }
            "any_of" => {
                self.any_of.push(value.to_string());
                Ok(())
            }
            "intitle" => {
                self.intitle.push(value.to_string());
                Ok(())
            }
            "intext" => {
                self.intext.push(value.to_string());
                Ok(())
            }
            "inurl" => {
                self.inurl.push(value.to_string());
                Ok(())
            }
            "filetype" => {
                self.filetype = Some(value.to_string());
                Ok(())
            }
            "after" => {
                self.after = Some(value.to_string());
                Ok(())
            }
            "before" => {
                self.before = Some(value.to_string());
                Ok(())
            }
            "range" => {
                self.range = Some(parse_range_value(value)?);
                Ok(())
            }
            _ => Err(format!("unknown field {field:?}")),
        }
    }

    /// The dork string. Order is fixed (terms, exact, any-of, sites, exclude,
    /// exclude-sites, related, filetype, intitle, intext, inurl, after,
    /// before, range) so the same struct always renders the same string — the
    /// dashboard diffs against it to know whether an edit changed anything.
    /// An empty struct renders `""`, never a dangling operator.
    pub fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        let terms = self.terms.trim();
        if !terms.is_empty() {
            parts.push(terms.to_string());
        }
        for e in &self.exact {
            parts.push(format!("\"{e}\""));
        }
        if !self.any_of.is_empty() {
            parts.push(format!("({})", self.any_of.join(" OR ")));
        }
        match self.sites.len() {
            0 => {}
            1 => parts.push(format!("site:{}", self.sites[0])),
            _ => parts.push(format!(
                "({})",
                self.sites.iter().map(|s| format!("site:{s}")).collect::<Vec<_>>().join(" OR ")
            )),
        }
        for x in &self.exclude {
            parts.push(negate(x));
        }
        for s in &self.exclude_sites {
            parts.push(format!("-site:{s}"));
        }
        for r in &self.related {
            parts.push(format!("related:{r}"));
        }
        if let Some(ft) = &self.filetype {
            parts.push(format!("filetype:{ft}"));
        }
        for t in &self.intitle {
            parts.push(format!("intitle:{}", quote_if_spaced(t)));
        }
        for t in &self.intext {
            parts.push(format!("intext:{}", quote_if_spaced(t)));
        }
        for u in &self.inurl {
            parts.push(format!("inurl:{}", quote_if_spaced(u)));
        }
        if let Some(a) = &self.after {
            parts.push(format!("after:{a}"));
        }
        if let Some(b) = &self.before {
            parts.push(format!("before:{b}"));
        }
        if let Some((lo, hi)) = &self.range {
            parts.push(format!("{lo}..{hi}"));
        }

        parts.join(" ")
    }

    /// The inverse of [`Self::render`] — what lets a user type raw dork into
    /// the box and get the same chips back. Tokenizes on whitespace, keeping a
    /// `"quoted phrase"` and a balanced `(a OR b)` group as one token each,
    /// then classifies every token by its operator prefix; anything left over
    /// joins back into `terms`.
    pub fn parse(s: &str) -> DorkQuery {
        let mut q = DorkQuery::default();
        let mut terms: Vec<String> = Vec::new();

        for token in tokenize(s) {
            if let Some(inner) = strip_quoted(&token) {
                if token.starts_with('-') {
                    q.exclude.push(inner);
                } else {
                    q.exact.push(inner);
                }
                continue;
            }
            if token.starts_with('(') && token.ends_with(')') && token.len() >= 2 {
                let inner = &token[1..token.len() - 1];
                let items: Vec<&str> =
                    inner.split(" OR ").map(str::trim).filter(|s| !s.is_empty()).collect();
                if !items.is_empty() && items.iter().all(|it| it.starts_with("site:")) {
                    for it in items {
                        let _ = q.add_site(it.trim_start_matches("site:"));
                    }
                } else if !items.is_empty() {
                    q.any_of = items.into_iter().map(str::to_string).collect();
                }
                continue;
            }
            if let Some(v) = token.strip_prefix("-site:") {
                let _ = q.add_exclude_site(v);
            } else if let Some(v) = token.strip_prefix("site:") {
                let _ = q.add_site(v);
            } else if let Some(v) = token.strip_prefix("related:") {
                let _ = q.add_related(v);
            } else if let Some(v) = token.strip_prefix("filetype:") {
                q.filetype = Some(v.to_string());
            } else if let Some(v) = token.strip_prefix("ext:") {
                // `ext:` is Google's synonym for `filetype:` — accepted here
                // so a dork pasted from elsewhere doesn't lose the operator
                // into `terms`, but `render` only ever emits the canonical
                // `filetype:` spelling (this is a one-way alias, not a
                // second operator to keep in sync — see the module doc
                // comment's note on `allintitle:` etc for why).
                q.filetype = Some(v.to_string());
            } else if let Some(v) = token.strip_prefix("intitle:") {
                q.intitle.push(unquote(v));
            } else if let Some(v) = token.strip_prefix("intext:") {
                q.intext.push(unquote(v));
            } else if let Some(v) = token.strip_prefix("inurl:") {
                q.inurl.push(unquote(v));
            } else if let Some(v) = token.strip_prefix("after:") {
                q.after = Some(v.to_string());
            } else if let Some(v) = token.strip_prefix("before:") {
                q.before = Some(v.to_string());
            } else if let Some(caps) = RANGE_TOKEN.captures(&token) {
                q.range = Some((caps[1].to_string(), caps[2].to_string()));
            } else if let Some(v) = token.strip_prefix('-') {
                q.exclude.push(v.to_string());
            } else {
                terms.push(token);
            }
        }

        q.terms = terms.join(" ");
        q
    }

    /// One line per operator actually in play, in [`Self::render`]'s own
    /// order — the teaching feature `docs/web-search-module-plan.md` calls
    /// "the feature, not decoration". Plain `terms` is not an operator and has
    /// no line; everything else `render` can emit has one here.
    pub fn explain(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();

        for e in &self.exact {
            out.push((format!("\"{e}\""), format!("must contain the exact phrase \"{e}\"")));
        }
        if !self.any_of.is_empty() {
            let op = format!("({})", self.any_of.join(" OR "));
            out.push((op, format!("matches at least one of: {}", self.any_of.join(", "))));
        }
        match self.sites.len() {
            0 => {}
            1 => {
                let site = &self.sites[0];
                out.push((format!("site:{site}"), format!("only pages on {site}")));
            }
            _ => {
                let op = format!(
                    "({})",
                    self.sites.iter().map(|s| format!("site:{s}")).collect::<Vec<_>>().join(" OR ")
                );
                out.push((op, format!("only pages on one of: {}", self.sites.join(", "))));
            }
        }
        for x in &self.exclude {
            out.push((negate(x), format!("excludes pages that mention \"{x}\"")));
        }
        for s in &self.exclude_sites {
            out.push((format!("-site:{s}"), format!("excludes pages on {s}")));
        }
        for r in &self.related {
            out.push((format!("related:{r}"), format!("sites similar to {r}")));
        }
        if let Some(ft) = &self.filetype {
            out.push((format!("filetype:{ft}"), format!("only results of file type {ft}")));
        }
        for t in &self.intitle {
            out.push((
                format!("intitle:{}", quote_if_spaced(t)),
                format!("the page title must contain \"{t}\""),
            ));
        }
        for t in &self.intext {
            out.push((
                format!("intext:{}", quote_if_spaced(t)),
                format!("the page text must contain \"{t}\""),
            ));
        }
        for u in &self.inurl {
            out.push((
                format!("inurl:{}", quote_if_spaced(u)),
                format!("the page address must contain \"{u}\""),
            ));
        }
        if let Some(a) = &self.after {
            out.push((format!("after:{a}"), format!("only results published after {a}")));
        }
        if let Some(b) = &self.before {
            out.push((format!("before:{b}"), format!("only results published before {b}")));
        }
        if let Some((lo, hi)) = &self.range {
            out.push((format!("{lo}..{hi}"), format!("a number between {lo} and {hi}")));
        }

        out
    }

    /// Removes the one piece `token` addresses, matched against exactly what
    /// [`Self::render`] would emit for that piece — a `site:`/`filetype:`/…
    /// spelling for the fields that have one, or the bare exclude-quoted
    /// spelling (`-x`, `-"a b"`) for `exclude`. This is `render`'s inverse in
    /// the same sense [`Self::parse`] is, which is why it lives beside both:
    /// a client only ever has to send back a token this struct itself
    /// produced (a chip label), never re-derive the operator grammar.
    ///
    /// Multi-value fields (`sites`, `exclude`, `exact`, `intitle`, `intext`,
    /// `inurl`, `exclude_sites`, `related`, `any_of`) drop the one matching
    /// element, not the whole field — `render` collapsing several sites into
    /// one `(site:a OR site:b)` group is exactly why a single site must still
    /// be addressable as `site:a` on its own. `any_of` has no operator prefix
    /// of its own, so its element matches on the bare term.
    ///
    /// An unmatched token is a no-op: returns `false`, changes nothing. The
    /// caller (`search.rs`) treats that as "still a 200", not an error — the
    /// box may have been edited since the token was produced.
    pub fn drop_part(&mut self, token: &str) -> bool {
        let token = token.trim();
        if token.is_empty() {
            return false;
        }
        if let Some(i) = self.exact.iter().position(|e| format!("\"{e}\"") == token) {
            self.exact.remove(i);
            return true;
        }
        if let Some(i) = self.any_of.iter().position(|a| a == token) {
            self.any_of.remove(i);
            return true;
        }
        if let Some(i) = self.sites.iter().position(|s| format!("site:{s}") == token) {
            self.sites.remove(i);
            return true;
        }
        if let Some(i) = self.exclude.iter().position(|x| negate(x) == token) {
            self.exclude.remove(i);
            return true;
        }
        if let Some(i) = self.exclude_sites.iter().position(|s| format!("-site:{s}") == token) {
            self.exclude_sites.remove(i);
            return true;
        }
        if let Some(i) = self.related.iter().position(|r| format!("related:{r}") == token) {
            self.related.remove(i);
            return true;
        }
        if self.filetype.as_deref().is_some_and(|ft| format!("filetype:{ft}") == token) {
            self.filetype = None;
            return true;
        }
        if let Some(i) =
            self.intitle.iter().position(|t| format!("intitle:{}", quote_if_spaced(t)) == token)
        {
            self.intitle.remove(i);
            return true;
        }
        if let Some(i) =
            self.intext.iter().position(|t| format!("intext:{}", quote_if_spaced(t)) == token)
        {
            self.intext.remove(i);
            return true;
        }
        if let Some(i) = self.inurl.iter().position(|u| format!("inurl:{}", quote_if_spaced(u)) == token)
        {
            self.inurl.remove(i);
            return true;
        }
        if self.after.as_deref().is_some_and(|a| format!("after:{a}") == token) {
            self.after = None;
            return true;
        }
        if self.before.as_deref().is_some_and(|b| format!("before:{b}") == token) {
            self.before = None;
            return true;
        }
        if self.range.as_ref().is_some_and(|(lo, hi)| format!("{lo}..{hi}") == token) {
            self.range = None;
            return true;
        }
        false
    }

    /// One chip per removable element — [`Self::render`]'s own field order,
    /// and a grouped `(site:a OR site:b)` render still yields one chip per
    /// site, matching what [`Self::drop_part`] can address (never one chip
    /// per collapsed group). Every `token` here is built with the exact same
    /// helpers `render`/`drop_part` use (`negate`, `quote_if_spaced`), so a
    /// caller can always drop a chip this struct itself produced — that
    /// equivalence is what `chips_are_exactly_what_drop_part_can_remove`
    /// below proves. `label` is the same value with its operator prefix
    /// stripped for display (`reddit.com`, not `site:reddit.com`); tone is a
    /// client concern and deliberately not part of this shape.
    pub fn chips(&self) -> Vec<DorkChip> {
        let mut out = Vec::new();
        for e in &self.exact {
            out.push(DorkChip { token: format!("\"{e}\""), label: e.clone(), field: "exact" });
        }
        for a in &self.any_of {
            out.push(DorkChip { token: a.clone(), label: format!("or {a}"), field: "any_of" });
        }
        for s in &self.sites {
            out.push(DorkChip { token: format!("site:{s}"), label: s.clone(), field: "sites" });
        }
        for x in &self.exclude {
            out.push(DorkChip { token: negate(x), label: x.clone(), field: "exclude" });
        }
        for s in &self.exclude_sites {
            out.push(DorkChip {
                token: format!("-site:{s}"),
                label: s.clone(),
                field: "exclude_sites",
            });
        }
        for r in &self.related {
            out.push(DorkChip { token: format!("related:{r}"), label: r.clone(), field: "related" });
        }
        if let Some(ft) = &self.filetype {
            out.push(DorkChip { token: format!("filetype:{ft}"), label: ft.clone(), field: "filetype" });
        }
        for t in &self.intitle {
            out.push(DorkChip {
                token: format!("intitle:{}", quote_if_spaced(t)),
                label: t.clone(),
                field: "intitle",
            });
        }
        for t in &self.intext {
            out.push(DorkChip {
                token: format!("intext:{}", quote_if_spaced(t)),
                label: t.clone(),
                field: "intext",
            });
        }
        for u in &self.inurl {
            out.push(DorkChip {
                token: format!("inurl:{}", quote_if_spaced(u)),
                label: u.clone(),
                field: "inurl",
            });
        }
        if let Some(a) = &self.after {
            out.push(DorkChip { token: format!("after:{a}"), label: a.clone(), field: "after" });
        }
        if let Some(b) = &self.before {
            out.push(DorkChip { token: format!("before:{b}"), label: b.clone(), field: "before" });
        }
        if let Some((lo, hi)) = &self.range {
            // No operator prefix to strip — the token itself is already the
            // display label, same as `any_of`'s bare term.
            out.push(DorkChip { token: format!("{lo}..{hi}"), label: format!("{lo}..{hi}"), field: "range" });
        }
        out
    }

    /// The ready-to-open URL. Percent-encoded via `url`'s
    /// `application/x-www-form-urlencoded` serializer — the same encoding a
    /// browser's own search box submits, which is why a space becomes `+`
    /// rather than `%20`.
    pub fn url(&self, engine: Engine) -> String {
        let query: String = url::form_urlencoded::byte_serialize(self.render().as_bytes()).collect();
        format!("{}{}", engine.base_url(), query)
    }
}

fn negate(term: &str) -> String {
    format!("-{}", quote_if_spaced(term))
}

/// A bare `lo..hi` token — Google's numeric range operator — recognised by
/// [`DorkQuery::parse`] so it round-trips through the dedicated `range` field
/// instead of falling into `terms` as an opaque string.
static RANGE_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)\.\.(\d+)$").unwrap());

/// [`DorkQuery::add_part`]'s `range` arm: accepts `lo..hi` or `lo-hi`, two
/// plain non-negative integers. Anything else is a named 400, per the route's
/// contract that a failed *add* must not be a silent no-op.
fn parse_range_value(value: &str) -> Result<(String, String), String> {
    let (lo, hi) = value
        .split_once("..")
        .or_else(|| value.split_once('-'))
        .ok_or_else(|| format!("range needs \"lo..hi\" or \"lo-hi\", got {value:?}"))?;
    let (lo, hi) = (lo.trim(), hi.trim());
    let both_numeric = !lo.is_empty()
        && !hi.is_empty()
        && lo.chars().all(|c| c.is_ascii_digit())
        && hi.chars().all(|c| c.is_ascii_digit());
    if !both_numeric {
        return Err(format!("range needs two numbers, got {value:?}"));
    }
    Ok((lo.to_string(), hi.to_string()))
}

fn quote_if_spaced(s: &str) -> String {
    if s.chars().any(char::is_whitespace) {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// A bare `"phrase"` or `-"phrase"` token, unwrapped. `None` for anything else
/// (including `intitle:"phrase"`, which `unquote` handles instead) — the
/// distinction is whether the *whole* token is the quoted span or an operator
/// carries it.
fn strip_quoted(token: &str) -> Option<String> {
    let body = token.strip_prefix('-').unwrap_or(token);
    if body.len() >= 2 && body.starts_with('"') && body.ends_with('"') {
        Some(body[1..body.len() - 1].to_string())
    } else {
        None
    }
}

/// Splits on whitespace, except inside a `"…"` span (which may itself sit
/// inside a larger token like `-"a b"` or `intitle:"a b"`) or a balanced
/// `(…)` group — both of which stay one token so [`DorkQuery::parse`] can
/// classify them as a unit.
fn tokenize(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '(' {
            let start = i;
            let mut depth = 0i32;
            while i < chars.len() {
                match chars[i] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        i += 1;
                        if depth <= 0 {
                            break;
                        }
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            if chars[i] == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                if i < chars.len() {
                    i += 1; // the closing quote
                }
            } else {
                i += 1;
            }
        }
        tokens.push(chars[start..i].iter().collect());
    }
    tokens
}

/// The engine table ADR 0008 calls "a URL template, so supporting three costs
/// a three-row table rather than three integrations" — every row answers `q=`,
/// which is what makes it three rows and not three special cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Google,
    #[serde(rename = "duckduckgo")]
    DuckDuckGo,
    Bing,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::Google
    }
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Google => "google",
            Engine::DuckDuckGo => "duckduckgo",
            Engine::Bing => "bing",
        }
    }

    /// `?engine=` from the route, defaulting to Google rather than rejecting
    /// an unknown value — a typo in a query param should not turn a search
    /// into a 400.
    pub fn parse(s: &str) -> Option<Engine> {
        match s.trim().to_ascii_lowercase().as_str() {
            "google" => Some(Engine::Google),
            "duckduckgo" | "ddg" => Some(Engine::DuckDuckGo),
            "bing" => Some(Engine::Bing),
            _ => None,
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Engine::Google => "https://www.google.com/search?q=",
            Engine::DuckDuckGo => "https://duckduckgo.com/?q=",
            Engine::Bing => "https://www.bing.com/search?q=",
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 1's rule translator — a table of named intent recipes
// ---------------------------------------------------------------------------
//
// Each [`Recipe`] owns one slice of intent: a set of trigger phrases, a
// function that mutates a [`DorkQuery`] and strips what it consumed from the
// remaining text, and a plain-English line for the UI. Recipes are additive
// and independent — [`DorkQuery::from_phrases`] runs every row over the same
// text and lets however many fire, fire. `onsite`, `recent` and `exclusion`
// are not special-cased as "modifiers" in code; they are additive by
// construction like everything else, which is what makes composing with a
// content recipe (or with each other) fall out for free.

macro_rules! pattern {
    ($name:ident, $re:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($re).unwrap());
    };
}

/// Removes `re`'s first match from `text` in place and returns it, mapped
/// through `f`. The building block every recipe strips input with, so a
/// recipe's own leftover text — after every recipe has had a turn — is what
/// [`DorkQuery::from_phrases`] joins back into `terms`.
fn take_first(text: &mut String, re: &Regex, f: impl FnOnce(&str) -> String) -> Option<String> {
    let m = re.find(text)?;
    let (start, end) = (m.start(), m.end());
    let out = f(&text[start..end]);
    text.replace_range(start..end, "");
    Some(out)
}

/// Same, but for a pattern with one capture group — returns the group, not
/// the whole match, while still removing the whole match.
fn take_capture(text: &mut String, re: &Regex) -> Option<String> {
    let caps = re.captures(text)?;
    let whole = caps.get(0)?;
    let group = caps.get(1).map(|m| m.as_str().to_string());
    let (start, end) = (whole.start(), whole.end());
    text.replace_range(start..end, "");
    group
}

/// Same, for a pattern with two capture groups (a numeric range).
fn take_pair(text: &mut String, re: &Regex) -> Option<(String, String)> {
    let caps = re.captures(text)?;
    let whole = caps.get(0)?;
    let a = caps.get(1)?.as_str().to_string();
    let b = caps.get(2)?.as_str().to_string();
    let (start, end) = (whole.start(), whole.end());
    text.replace_range(start..end, "");
    Some((a, b))
}

/// Removes every match of `re` from `text`, in place. Returns whether there
/// was one — most recipes only care that their trigger word was present.
fn strip_all(text: &mut String, re: &Regex) -> bool {
    if re.is_match(text) {
        *text = re.replace_all(text, "").to_string();
        true
    } else {
        false
    }
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pattern!(QUOTED, r#""([^"]*)""#);

/// Not a recipe — a universal pre-pass. A quoted span is always an exact
/// phrase; `document`'s title handling (below) *promotes* one of these into
/// `intitle` afterwards when a title marker is also present, rather than
/// re-deriving quote extraction itself.
fn extract_exact(q: &mut DorkQuery, remaining: &mut String) {
    while let Some(phrase) = take_capture(remaining, &QUOTED) {
        if !phrase.trim().is_empty() {
            q.exact.push(phrase);
        }
    }
}

// --- document ---------------------------------------------------------------

pattern!(DOC_EXT_WORD, r"(?i)\b(docx|doc|pptx|ppt|xlsx|xls|pdf|csv|epub)\b");
pattern!(DOC_GENERIC_WORD, r"(?i)\b(?:manual|datasheet|spec sheet|whitepaper|ebook)s?\b");
pattern!(TITLE_TARGET, r"(?i)\b(?:titled|called|named)\s+([A-Za-z][\w-]*)");
pattern!(TITLE_WORD, r"(?i)\b(?:title|titled|called|named)\b");

/// "find a pdf with this title …", "the datasheet for …", "paper titled …".
/// `filetype` and `intitle` are set independently — a title marker means
/// "match the title", not "this is a PDF". Only an extension word (`pdf`,
/// `docx`, …) or a generic document word (manual/datasheet/whitepaper/ebook,
/// which do genuinely imply a document) sets `filetype`; a title marker sets
/// `intitle` regardless of whether either of those also fired. "the article
/// called Foo" is `intitle:Foo` with no `filetype:` at all — forcing
/// `filetype:pdf` on a bare title marker previously discarded every
/// non-PDF result the user actually wanted.
fn apply_document(q: &mut DorkQuery, remaining: &mut String) {
    let ext = take_first(remaining, &DOC_EXT_WORD, |m| m.to_lowercase());
    let generic = strip_all(remaining, &DOC_GENERIC_WORD);
    // "titled <word>" is consumed as one unit so the anchor and its target
    // leave together; a bare marker (`title`/`titled`/`called`/`named` with
    // nothing unambiguous after it — the quoted-title case) falls through to
    // the plain strip below.
    let bare_title = take_capture(remaining, &TITLE_TARGET);
    let has_title_word = bare_title.is_some() || strip_all(remaining, &TITLE_WORD);

    if ext.is_none() && !generic && !has_title_word {
        return;
    }

    if ext.is_some() || generic {
        q.filetype.get_or_insert_with(|| ext.unwrap_or_else(|| "pdf".to_string()));
    }

    if has_title_word {
        if let Some(quoted) = q.exact.pop() {
            q.intitle.push(quoted);
        } else if let Some(word) = bare_title {
            q.intitle.push(word);
        }
    }
}

// --- discussion --------------------------------------------------------------

pattern!(
    DISCUSSION_WORD,
    r"(?i)\b(?:reviews?|opinions?|reddit|forum|discussions?|what do people think)\b"
);
const DISCUSSION_SITES: [&str; 3] = ["reddit.com", "news.ycombinator.com", "stackexchange.com"];

fn apply_discussion(q: &mut DorkQuery, remaining: &mut String) {
    if !strip_all(remaining, &DISCUSSION_WORD) {
        return;
    }
    for site in DISCUSSION_SITES {
        let _ = q.add_site(site);
    }
}

// --- academic ------------------------------------------------------------

pattern!(ACADEMIC_WORD, r"(?i)\b(?:papers?|stud(?:y|ies)|research|journal|citations?)\b");
// Narrower than ACADEMIC_WORD: only the words that name the *artifact* you'd
// want as a PDF. "citation" and "research" name the topic, not the format —
// "recent research on X" is not a request for a PDF, and forcing
// filetype:pdf on it is the same class of bug as apply_document's bare title
// marker above. "paper"/"study"/"journal" do imply a document; those still
// get filetype:pdf. The recipe still fires and still restricts to academic
// sites on any ACADEMIC_WORD — only the filetype restriction is narrowed.
pattern!(ACADEMIC_DOC_WORD, r"(?i)\b(?:papers?|stud(?:y|ies)|journal)\b");
// ponytail: `.edu` here has the same US-centric shape as the old
// SHOPPING_SITES list below, but it's additive alongside arxiv.org (which
// covers the general case) rather than the only site restriction, so it's
// far less harmful and is left as-is.
const ACADEMIC_SITES: [&str; 3] = ["arxiv.org", "scholar.google.com", ".edu"];

fn apply_academic(q: &mut DorkQuery, remaining: &mut String) {
    let wants_pdf = ACADEMIC_DOC_WORD.is_match(remaining);
    if !strip_all(remaining, &ACADEMIC_WORD) {
        return;
    }
    if wants_pdf {
        q.filetype.get_or_insert_with(|| "pdf".to_string());
    }
    for site in ACADEMIC_SITES {
        let _ = q.add_site(site);
    }
}

// --- docs ------------------------------------------------------------------

pattern!(DOCS_WORD, r"(?i)\b(?:api docs|documentation|reference|how to use)\b");

fn apply_docs(q: &mut DorkQuery, remaining: &mut String) {
    if !strip_all(remaining, &DOCS_WORD) {
        return;
    }
    q.inurl.push("docs".to_string());
}

// --- shopping ----------------------------------------------------------------

pattern!(SHOPPING_WORD, r"(?i)\b(?:cheap(?:est)?|prices?|deals?|buy|under)\b");
// Only a *pair* of bounds becomes a range — Google's `100..200` operator. A
// single bound ("under 200") stays a plain term rather than a guessed range.
pattern!(PRICE_RANGE, r"(?i)(?:between\s+)?[£$]?(\d+)\s*(?:-|to|and)\s*[£$]?(\d+)\b");
// `site:amazon.com` does not match amazon.co.uk — a US-only list silently
// searches the wrong storefronts for anyone outside the US (this file's own
// PRICE_RANGE regex already accepts `£`, so the author knew there were
// non-US users). Google's `site:` wants a full registrable domain, so there
// is no TLD-agnostic spelling; the honest fix is both TLD variants per
// retailer rather than swapping one hardcoded country for another.
//
// ponytail: still hardcoded (amazon + ebay, one TLD each side of the
// Atlantic) — upgrade path is a locale setting that picks the TLD, or a
// user-configurable retailer list. Walmart has no non-US storefront, so it's
// dropped rather than shipped US-only again.
const SHOPPING_SITES: [&str; 4] = ["amazon.com", "amazon.co.uk", "ebay.com", "ebay.co.uk"];
// A literal currency symbol in the ask ("under $200") is worth requiring in
// the page body too — it is what tells an actual price listing apart from a
// blog post that merely mentions the product. Same symbol set PRICE_RANGE
// already recognises; optional, never forced when the ask gives no symbol.
pattern!(CURRENCY_SYMBOL, r"[£$]");

fn apply_shopping(q: &mut DorkQuery, remaining: &mut String) {
    if !strip_all(remaining, &SHOPPING_WORD) {
        return;
    }
    for site in SHOPPING_SITES {
        let _ = q.add_site(site);
    }
    if let Some(sym) = take_first(remaining, &CURRENCY_SYMBOL, |m| m.to_string()) {
        q.intext.push(sym);
    }
    if let Some((lo, hi)) = take_pair(remaining, &PRICE_RANGE) {
        q.range = Some((lo, hi));
    }
}

// --- onsite (modifier) -------------------------------------------------------

// Requires a dot in the captured token, so "reviews on top" or "answers from
// friends" do not misread as a domain — only something that looks like one.
pattern!(SITE_PHRASE, r"(?i)\b(?:on|from|site)\s+([\w-]+(?:\.[\w-]+)+)");

fn apply_onsite(q: &mut DorkQuery, remaining: &mut String) {
    while let Some(domain) = take_capture(remaining, &SITE_PHRASE) {
        let _ = q.add_site(&domain);
    }
}

// --- recent (modifier) -------------------------------------------------------

pattern!(SINCE_YEAR, r"(?i)\bsince\s+(\d{4})\b");
pattern!(LAST_OR_PAST_YEAR, r"(?i)\b(?:last|past)\s+year\b");
pattern!(THIS_YEAR_OR_RECENT, r"(?i)\b(?:this\s+year|recent)\b");

fn apply_recent(q: &mut DorkQuery, remaining: &mut String) {
    if let Some(year) = take_capture(remaining, &SINCE_YEAR) {
        q.after = Some(format!("{year}-01-01"));
        return;
    }
    let this_year = chrono::Utc::now().year();
    if strip_all(remaining, &LAST_OR_PAST_YEAR) {
        q.after = Some(format!("{}-01-01", this_year - 1));
    } else if strip_all(remaining, &THIS_YEAR_OR_RECENT) {
        q.after = Some(format!("{this_year}-01-01"));
    }
}

// --- exclusion (modifier) -----------------------------------------------------

pattern!(EXCLUDE_PHRASE, r"(?i)\b(?:not|excluding|without)\s+([a-zA-Z][\w-]*)");
pattern!(HYPHEN_EXCLUDE, r"(?:^|\s)-([A-Za-z][\w-]*)");

fn apply_exclusion(q: &mut DorkQuery, remaining: &mut String) {
    while let Some(word) = take_capture(remaining, &EXCLUDE_PHRASE) {
        q.exclude.push(word);
    }
    while let Some(word) = take_capture(remaining, &HYPHEN_EXCLUDE) {
        q.exclude.push(word);
    }
}

/// One row of the recipe table. `triggers` is what the tests below walk, both
/// to prove the recipe fires on each of its own phrases and, generically,
/// that every row shipped with a name for the UI to describe itself with.
struct Recipe {
    name: &'static str,
    // Only read from `#[cfg(test)]` today — the table is walked by the
    // "fires on its own triggers" tests below, not by any runtime code path.
    #[allow(dead_code)]
    triggers: &'static [&'static str],
    apply: fn(&mut DorkQuery, &mut String),
    describes: &'static str,
}

/// The whole table. **This list, not more** — it is not a plugin system, and
/// it deliberately excludes anything exposure- or credential-hunting shaped
/// (see the module doc comment).
const RECIPES: &[Recipe] = &[
    Recipe {
        name: "document",
        triggers: &[
            "pdf",
            "manual",
            "datasheet",
            "spec sheet",
            "whitepaper",
            "ebook",
            "paper titled Foo",
            "document called Foo",
        ],
        apply: apply_document,
        describes: "Looking for a document — restricted to that file type, with a title match when one is given.",
    },
    Recipe {
        name: "discussion",
        triggers: &["reviews", "opinions", "what do people think", "reddit", "forum", "discussion"],
        apply: apply_discussion,
        describes: "Looking for discussion — restricted to Reddit, Hacker News and Stack Exchange.",
    },
    Recipe {
        name: "academic",
        triggers: &["paper", "study", "research", "journal", "citation"],
        apply: apply_academic,
        describes: "Looking for academic work — restricted to PDFs on arXiv, .edu sites and Google Scholar.",
    },
    Recipe {
        name: "docs",
        triggers: &["api docs", "documentation", "reference", "how to use"],
        apply: apply_docs,
        describes: "Looking for reference documentation — restricted to documentation paths.",
    },
    Recipe {
        name: "shopping",
        triggers: &["cheap", "cheapest", "price", "deal", "buy", "under 200"],
        apply: apply_shopping,
        describes: "Looking to buy — restricted to a handful of shopping sites, plus a price range or a required currency symbol when the ask gives one.",
    },
    Recipe {
        name: "onsite",
        triggers: &["on example.com", "from example.com", "site example.com"],
        apply: apply_onsite,
        describes: "Restricted to a specific site.",
    },
    Recipe {
        name: "recent",
        triggers: &["since 2024", "last year", "past year", "this year", "recent"],
        apply: apply_recent,
        describes: "Restricted to results published after a date.",
    },
    Recipe {
        name: "exclusion",
        triggers: &["not spam", "excluding spam", "without spam", "-spam"],
        apply: apply_exclusion,
        describes: "Excludes pages that mention a term.",
    },
];

/// The plain-English line for a fired recipe, by name — what `search.rs` puts
/// ahead of [`DorkQuery::explain`]'s per-operator lines in the response.
pub fn recipe_describes(name: &str) -> Option<&'static str> {
    RECIPES.iter().find(|r| r.name == name).map(|r| r.describes)
}

impl DorkQuery {
    /// The deterministic, offline translator. Every recipe in [`RECIPES`] runs
    /// over the same text and contributes whatever it finds; the return value
    /// is the query plus the names of the recipes that actually fired (a
    /// recipe "fires" when it leaves the struct different from how it found
    /// it). `search.rs` calls the model (phase 3) only when that list is
    /// empty — "the rules found nothing beyond plain terms".
    pub fn from_phrases(ask: &str) -> (DorkQuery, Vec<&'static str>) {
        let mut q = DorkQuery::default();
        let mut remaining = ask.to_string();
        extract_exact(&mut q, &mut remaining);

        let mut fired = Vec::new();
        for recipe in RECIPES {
            let before = q.clone();
            (recipe.apply)(&mut q, &mut remaining);
            if q != before {
                fired.push(recipe.name);
            }
        }

        q.terms = normalize_whitespace(&remaining);
        (q, fired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_recipe(name: &str) -> &'static Recipe {
        RECIPES.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no recipe named {name}"))
    }

    /// A phrase none of the eight recipes should ever fire on — the negative
    /// control for every "and on nothing else" test below.
    const NEUTRAL: &str = "blue widgets for sale";

    #[test]
    fn empty_query_renders_empty() {
        assert_eq!(DorkQuery::default().render(), "");
        assert_eq!(DorkQuery::parse("").render(), "");
        assert_eq!(DorkQuery::parse("   ").render(), "");
    }

    #[test]
    fn render_and_parse_round_trip() {
        let cases = vec![
            DorkQuery::default(),
            DorkQuery { terms: "keyboard".into(), ..Default::default() },
            DorkQuery {
                terms: "cheap mechanical keyboard".into(),
                sites: vec!["reddit.com".into()],
                exclude: vec!["membrane".into()],
                ..Default::default()
            },
            DorkQuery { sites: vec!["amazon.com".into(), "ebay.com".into()], ..Default::default() },
            DorkQuery { exact: vec!["cherry mx switches".into()], ..Default::default() },
            DorkQuery { any_of: vec!["red".into(), "blue".into()], ..Default::default() },
            DorkQuery {
                filetype: Some("pdf".into()),
                intitle: vec!["setup guide".into()],
                inurl: vec!["forum".into()],
                after: Some("2024-01-01".into()),
                before: Some("2024-12-31".into()),
                ..Default::default()
            },
            DorkQuery {
                terms: "router".into(),
                exclude_sites: vec!["pinterest.com".into()],
                ..Default::default()
            },
            DorkQuery { intext: vec!["cherry mx switches".into()], ..Default::default() },
            DorkQuery { related: vec!["example.com".into()], ..Default::default() },
            DorkQuery { range: Some(("100".into(), "200".into())), ..Default::default() },
            DorkQuery {
                terms: "keyboard".into(),
                intext: vec!["hot swap".into()],
                related: vec!["example.com".into(), "example.org".into()],
                range: Some(("50".into(), "150".into())),
                ..Default::default()
            },
        ];
        for q in cases {
            let rendered = q.render();
            let parsed = DorkQuery::parse(&rendered);
            assert_eq!(parsed, q, "round trip failed for {rendered:?}");
        }
    }

    /// `ext:` is Google's synonym for `filetype:` — an asymmetric alias, not a
    /// second field: `parse` accepts it, but `render` always emits the
    /// canonical spelling, so a pasted `ext:pdf` still round-trips to the one
    /// form the rest of the module understands.
    #[test]
    fn ext_is_accepted_as_a_filetype_alias_but_never_rendered() {
        let q = DorkQuery::parse("ext:pdf");
        assert_eq!(q.filetype.as_deref(), Some("pdf"));
        assert_eq!(q.render(), "filetype:pdf");
    }

    #[test]
    fn a_site_with_whitespace_cannot_be_constructed() {
        let mut q = DorkQuery::default();
        assert!(q.add_site("red dit.com").is_err());
        assert!(q.sites.is_empty());
        assert!(q.add_exclude_site("pinter est.com").is_err());
        assert!(q.exclude_sites.is_empty());

        assert!(q.add_site("reddit.com").is_ok());
        assert_eq!(q.sites, vec!["reddit.com".to_string()]);

        q.sites.push("bad site.com".to_string());
        assert!(q.validate().is_err(), "validate() must catch what bypassed add_site");

        // related gets the same guard, both directly and via validate().
        assert!(q.add_related("rel ated.com").is_err());
        assert!(q.related.is_empty());
        assert!(q.add_related("example.com").is_ok());
        assert_eq!(q.related, vec!["example.com".to_string()]);
        q.related.push("bad related.com".to_string());
        assert!(q.validate().is_err(), "validate() must also catch a bad related site");
    }

    /// One case per `add_part` field — every name the route accepts — plus
    /// the guard rejections and the unknown-field case. Unlike `drop_part`,
    /// a failure here must be `Err`, never a silent no-op.
    #[test]
    fn add_part_covers_every_field() {
        let mut q = DorkQuery::default();
        assert!(q.add_part("sites", "reddit.com").is_ok());
        assert_eq!(q.sites, vec!["reddit.com".to_string()]);

        assert!(q.add_part("exclude_sites", "pinterest.com").is_ok());
        assert_eq!(q.exclude_sites, vec!["pinterest.com".to_string()]);

        assert!(q.add_part("related", "example.com").is_ok());
        assert_eq!(q.related, vec!["example.com".to_string()]);

        assert!(q.add_part("exclude", "membrane").is_ok());
        assert_eq!(q.exclude, vec!["membrane".to_string()]);

        assert!(q.add_part("exact", "cherry mx switches").is_ok());
        assert_eq!(q.exact, vec!["cherry mx switches".to_string()]);

        assert!(q.add_part("any_of", "red").is_ok());
        assert_eq!(q.any_of, vec!["red".to_string()]);

        assert!(q.add_part("intitle", "setup guide").is_ok());
        assert_eq!(q.intitle, vec!["setup guide".to_string()]);

        assert!(q.add_part("intext", "cherry mx").is_ok());
        assert_eq!(q.intext, vec!["cherry mx".to_string()]);

        assert!(q.add_part("inurl", "forum").is_ok());
        assert_eq!(q.inurl, vec!["forum".to_string()]);

        assert!(q.add_part("filetype", "pdf").is_ok());
        assert_eq!(q.filetype.as_deref(), Some("pdf"));

        assert!(q.add_part("after", "2024-01-01").is_ok());
        assert_eq!(q.after.as_deref(), Some("2024-01-01"));

        assert!(q.add_part("before", "2024-12-31").is_ok());
        assert_eq!(q.before.as_deref(), Some("2024-12-31"));

        assert!(q.add_part("range", "100..200").is_ok());
        assert_eq!(q.range, Some(("100".to_string(), "200".to_string())));
        // The `lo-hi` spelling is accepted too.
        assert!(q.add_part("range", "50-75").is_ok());
        assert_eq!(q.range, Some(("50".to_string(), "75".to_string())));

        // The domain-shaped fields reuse the whitespace guard — a failed add
        // is a named error, not a no-op.
        assert_eq!(
            q.add_part("sites", "red dit.com"),
            Err("site \"red dit.com\" contains whitespace".to_string())
        );
        assert_eq!(
            q.add_part("exclude_sites", "pinter est.com"),
            Err("site \"pinter est.com\" contains whitespace".to_string())
        );
        assert_eq!(
            q.add_part("related", "rel ated.com"),
            Err("site \"rel ated.com\" contains whitespace".to_string())
        );

        // range rejects anything that is not two numbers.
        assert!(q.add_part("range", "not-a-range").is_err());
        assert!(q.add_part("range", "100").is_err());
        assert!(q.add_part("range", "100..").is_err());

        // An empty value and an unknown field are both errors.
        assert!(q.add_part("sites", "").is_err());
        assert!(q.add_part("sites", "   ").is_err());
        assert!(q.add_part("nonsense_field", "value").is_err());
    }

    #[test]
    fn url_is_percent_encoded() {
        let q = DorkQuery {
            terms: "cheap mechanical keyboard".into(),
            sites: vec!["reddit.com".into()],
            exclude: vec!["membrane".into()],
            ..Default::default()
        };
        assert_eq!(
            q.url(Engine::Google),
            "https://www.google.com/search?q=cheap+mechanical+keyboard+site%3Areddit.com+-membrane"
        );
        assert_eq!(
            q.url(Engine::DuckDuckGo),
            "https://duckduckgo.com/?q=cheap+mechanical+keyboard+site%3Areddit.com+-membrane"
        );
        assert_eq!(
            q.url(Engine::Bing),
            "https://www.bing.com/search?q=cheap+mechanical+keyboard+site%3Areddit.com+-membrane"
        );
    }

    /// Walks a struct with every field populated: every group `render` can
    /// emit must have exactly one line in `explain`, and that line's operator
    /// string must actually appear in the rendered output. A new field added
    /// to the struct without a matching `explain` arm changes this count and
    /// fails here.
    #[test]
    fn explain_covers_every_operator_render_can_emit() {
        let q = DorkQuery {
            terms: "keyboard".into(),
            exact: vec!["cherry mx".into()],
            any_of: vec!["red".into(), "blue".into()],
            exclude: vec!["membrane".into()],
            sites: vec!["reddit.com".into(), "amazon.com".into()],
            exclude_sites: vec!["pinterest.com".into()],
            related: vec!["example.com".into()],
            filetype: Some("pdf".into()),
            intitle: vec!["review".into()],
            intext: vec!["cherry mx".into()],
            inurl: vec!["forum".into()],
            after: Some("2024-01-01".into()),
            before: Some("2024-12-31".into()),
            range: Some(("100".into(), "200".into())),
        };
        let rendered = q.render();
        let explanation = q.explain();
        // exact, any_of, sites, exclude, exclude_sites, related, filetype,
        // intitle, intext, inurl, after, before, range — thirteen operator
        // groups, thirteen lines. `terms` is not an operator and has no line.
        assert_eq!(explanation.len(), 13);
        for (op, meaning) in &explanation {
            assert!(rendered.contains(op.as_str()), "{op:?} missing from rendered {rendered:?}");
            assert!(!meaning.trim().is_empty());
        }
    }

    /// Every field `drop_part` knows about, dropped by the exact token
    /// `render()` would have emitted for it — including the two quoting
    /// cases (`exact`'s bare `"…"`, `intitle`'s `intitle:"…"` only when the
    /// value has a space) — plus the multi-value fields dropping one element
    /// and leaving the rest, and an unmatched token as a no-op.
    #[test]
    fn drop_part_removes_exactly_the_matching_element() {
        let mut q = DorkQuery {
            terms: "keyboard".into(),
            exact: vec!["cherry mx switches".into()],
            any_of: vec!["red".into(), "blue".into()],
            exclude: vec!["membrane".into(), "wired keyboard".into()],
            sites: vec!["reddit.com".into(), "amazon.com".into()],
            exclude_sites: vec!["pinterest.com".into()],
            related: vec!["example.com".into(), "example.org".into()],
            filetype: Some("pdf".into()),
            intitle: vec!["review".into(), "setup guide".into()],
            intext: vec!["cherry mx".into(), "hot swap".into()],
            inurl: vec!["forum".into()],
            after: Some("2024-01-01".into()),
            before: Some("2024-12-31".into()),
            range: Some(("100".into(), "200".into())),
        };

        // A single site out of several — the case the plan calls out: the
        // grouped `(site:a OR site:b)` string can't address one of them, so
        // `site:reddit.com` alone must.
        assert!(q.drop_part("site:reddit.com"));
        assert_eq!(q.sites, vec!["amazon.com".to_string()]);

        assert!(q.drop_part("\"cherry mx switches\""));
        assert!(q.exact.is_empty());

        assert!(q.drop_part("blue"));
        assert_eq!(q.any_of, vec!["red".to_string()]);

        // exclude: a bare word and a spaced (quoted) one.
        assert!(q.drop_part("-membrane"));
        assert!(q.drop_part("-\"wired keyboard\""));
        assert!(q.exclude.is_empty());

        assert!(q.drop_part("-site:pinterest.com"));
        assert!(q.exclude_sites.is_empty());

        assert!(q.drop_part("related:example.com"));
        assert!(q.drop_part("related:example.org"));
        assert!(q.related.is_empty());

        assert!(q.drop_part("filetype:pdf"));
        assert!(q.filetype.is_none());

        // intitle: unquoted single-word vs quoted spaced value.
        assert!(q.drop_part("intitle:review"));
        assert!(q.drop_part("intitle:\"setup guide\""));
        assert!(q.intitle.is_empty());

        // intext: same two quoting cases as intitle.
        assert!(q.drop_part("intext:\"cherry mx\""));
        assert!(q.drop_part("intext:\"hot swap\""));
        assert!(q.intext.is_empty());

        assert!(q.drop_part("inurl:forum"));
        assert!(q.inurl.is_empty());

        assert!(q.drop_part("after:2024-01-01"));
        assert!(q.after.is_none());

        assert!(q.drop_part("before:2024-12-31"));
        assert!(q.before.is_none());

        assert!(q.drop_part("100..200"));
        assert!(q.range.is_none());

        // terms is untouched by any of the above.
        assert_eq!(q.terms, "keyboard");
    }

    #[test]
    fn drop_part_on_an_unmatched_token_is_a_no_op() {
        let original = DorkQuery {
            terms: "keyboard".into(),
            sites: vec!["reddit.com".into()],
            exclude: vec!["membrane".into()],
            ..Default::default()
        };
        let mut q = original.clone();

        assert!(!q.drop_part("site:amazon.com"), "no such site — must not match reddit.com");
        assert!(!q.drop_part("nonsense"));
        assert!(!q.drop_part(""));
        assert!(!q.drop_part("   "));
        assert_eq!(q, original, "an unmatched token must change nothing");
    }

    /// The invariant that makes chips and removal provably the same grammar:
    /// every chip [`DorkQuery::chips`] emits must be droppable by its own
    /// `token`, must remove exactly one element (not zero, not a whole
    /// group), and dropping every chip in turn must leave nothing but
    /// `terms`. A field added to the struct without a matching `chips` arm
    /// changes the element count below and fails here, same shape as
    /// `explain_covers_every_operator_render_can_emit` above.
    #[test]
    fn chips_are_exactly_what_drop_part_can_remove() {
        fn element_count(q: &DorkQuery) -> usize {
            q.exact.len()
                + q.any_of.len()
                + q.exclude.len()
                + q.sites.len()
                + q.exclude_sites.len()
                + q.related.len()
                + q.intitle.len()
                + q.intext.len()
                + q.inurl.len()
                + q.filetype.is_some() as usize
                + q.after.is_some() as usize
                + q.before.is_some() as usize
                + q.range.is_some() as usize
        }

        let mut q = DorkQuery {
            terms: "keyboard".into(),
            exact: vec!["cherry mx switches".into()],
            any_of: vec!["red".into(), "blue".into()],
            exclude: vec!["membrane".into(), "wired keyboard".into()],
            sites: vec!["reddit.com".into(), "amazon.com".into()],
            exclude_sites: vec!["pinterest.com".into()],
            related: vec!["example.com".into(), "example.org".into()],
            filetype: Some("pdf".into()),
            intitle: vec!["review".into(), "setup guide".into()],
            intext: vec!["cherry mx".into(), "hot swap".into()],
            inurl: vec!["forum".into()],
            after: Some("2024-01-01".into()),
            before: Some("2024-12-31".into()),
            range: Some(("100".into(), "200".into())),
        };

        let chips = q.chips();
        assert_eq!(chips.len(), element_count(&q), "one chip per element, never per group");

        for chip in &chips {
            let before = element_count(&q);
            assert!(q.drop_part(&chip.token), "chip {chip:?} was not droppable by its own token");
            assert_eq!(
                element_count(&q),
                before - 1,
                "chip {chip:?} did not remove exactly one element"
            );
        }

        assert_eq!(q, DorkQuery { terms: "keyboard".into(), ..Default::default() });
    }

    #[test]
    fn engine_urls_all_answer_q() {
        assert!(Engine::Google.base_url().ends_with("?q="));
        assert!(Engine::DuckDuckGo.base_url().ends_with("?q="));
        assert!(Engine::Bing.base_url().ends_with("?q="));
        assert_eq!(Engine::parse("DuckDuckGo"), Some(Engine::DuckDuckGo));
        assert_eq!(Engine::parse("nonsense"), None);
    }

    #[test]
    fn plain_ask_fires_no_recipe() {
        let (q, fired) = DorkQuery::from_phrases("mechanical keyboard quality");
        assert!(fired.is_empty());
        assert_eq!(q, DorkQuery { terms: "mechanical keyboard quality".into(), ..Default::default() });
    }

    #[test]
    fn every_recipe_has_a_description_and_at_least_one_trigger() {
        for recipe in RECIPES {
            assert!(!recipe.describes.trim().is_empty(), "{} has no description", recipe.name);
            assert!(!recipe.triggers.is_empty(), "{} has no triggers", recipe.name);
            assert!(recipe_describes(recipe.name).is_some());
        }
    }

    #[test]
    fn document_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        for trigger in find_recipe("document").triggers {
            let (_, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"document"), "{trigger:?} should fire document, got {fired:?}");
        }
        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"document"));
    }

    #[test]
    fn discussion_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        for trigger in find_recipe("discussion").triggers {
            let (_, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"discussion"), "{trigger:?} should fire discussion, got {fired:?}");
        }
        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"discussion"));
    }

    #[test]
    fn academic_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        for trigger in find_recipe("academic").triggers {
            let (_, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"academic"), "{trigger:?} should fire academic, got {fired:?}");
        }
        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"academic"));
    }

    #[test]
    fn docs_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        for trigger in find_recipe("docs").triggers {
            let (_, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"docs"), "{trigger:?} should fire docs, got {fired:?}");
        }
        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"docs"));
    }

    #[test]
    fn shopping_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        for trigger in find_recipe("shopping").triggers {
            let (_, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"shopping"), "{trigger:?} should fire shopping, got {fired:?}");
        }
        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"shopping"));
    }

    #[test]
    fn shopping_recipe_only_ranges_a_pair_of_bounds_not_a_single_one() {
        let (q, _) = DorkQuery::from_phrases("cheap gaming chair under 200");
        assert!(q.range.is_none(), "a single bound must not become a guessed range");
        assert!(!q.terms.contains(".."), "and must never leak a bare range token into terms");

        let (q2, _) = DorkQuery::from_phrases("cheap gaming chair between 100 and 200");
        assert_eq!(q2.range, Some(("100".to_string(), "200".to_string())));
        assert!(!q2.terms.contains(".."), "the range is a field now, not a term");

        let rendered = q2.render();
        assert!(rendered.contains("100..200"), "{rendered:?}");
    }

    #[test]
    fn shopping_recipe_uses_a_currency_symbol_as_intext_when_the_ask_gives_one() {
        let (q, _) = DorkQuery::from_phrases("cheap gaming chair under 200");
        assert!(q.intext.is_empty(), "no currency symbol in the ask, nothing forced into intext");

        let (q, _) = DorkQuery::from_phrases("cheap gaming chair under $200");
        assert_eq!(q.intext, vec!["$".to_string()]);

        let (q, _) = DorkQuery::from_phrases("cheap gaming chair between £100 and £200");
        assert_eq!(q.intext, vec!["£".to_string()]);
        assert_eq!(q.range, Some(("100".to_string(), "200".to_string())));
    }

    #[test]
    fn onsite_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        for trigger in find_recipe("onsite").triggers {
            let (_, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"onsite"), "{trigger:?} should fire onsite, got {fired:?}");
        }
        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"onsite"));
        // "on top" has no dot, so it must not misread as a domain.
        let (_, fired) = DorkQuery::from_phrases("learn on the job");
        assert!(!fired.contains(&"onsite"));
    }

    #[test]
    fn recent_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        let this_year = chrono::Utc::now().year();
        for trigger in find_recipe("recent").triggers {
            let (q, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"recent"), "{trigger:?} should fire recent, got {fired:?}");
            assert!(q.after.is_some());
        }
        let (q, _) = DorkQuery::from_phrases("since 2024");
        assert_eq!(q.after.as_deref(), Some("2024-01-01"));
        let (q, _) = DorkQuery::from_phrases("last year");
        assert_eq!(q.after.as_deref(), Some(format!("{}-01-01", this_year - 1)).as_deref());

        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"recent"));
    }

    #[test]
    fn exclusion_recipe_fires_on_its_triggers_and_not_on_unrelated_text() {
        for trigger in find_recipe("exclusion").triggers {
            let (q, fired) = DorkQuery::from_phrases(trigger);
            assert!(fired.contains(&"exclusion"), "{trigger:?} should fire exclusion, got {fired:?}");
            assert_eq!(q.exclude, vec!["spam".to_string()]);
        }
        let (_, fired) = DorkQuery::from_phrases(NEUTRAL);
        assert!(!fired.contains(&"exclusion"));
    }

    /// The user's own example: no format word beyond "pdf", a quoted title
    /// carried by "title". The title must land in `intitle` only — not
    /// duplicated into `exact` — and the composed dork must carry both
    /// operators.
    #[test]
    fn document_recipe_handles_the_users_own_example() {
        let (q, fired) =
            DorkQuery::from_phrases(r#"find a pdf with this title "Attention Is All You Need""#);
        assert_eq!(q.filetype.as_deref(), Some("pdf"));
        assert_eq!(q.intitle, vec!["Attention Is All You Need".to_string()]);
        assert!(q.exact.is_empty(), "the title moved into intitle, not duplicated into exact");
        assert!(fired.contains(&"document"));

        let rendered = q.render();
        assert!(rendered.contains("filetype:pdf"), "{rendered:?}");
        assert!(rendered.contains("intitle:\"Attention Is All You Need\""), "{rendered:?}");
    }

    /// Regression for the bare-title-forces-pdf bug: a title marker with no
    /// extension word and no generic document word must set `intitle` only.
    /// "the article called Foo Bar" is not a request for a PDF.
    #[test]
    fn a_bare_title_marker_does_not_force_filetype_pdf() {
        let (q, fired) = DorkQuery::from_phrases(r#"the article called "Foo Bar""#);
        assert_eq!(q.intitle, vec!["Foo Bar".to_string()]);
        assert!(q.filetype.is_none(), "a title marker alone must not imply a file type");
        assert!(fired.contains(&"document"));

        let rendered = q.render();
        assert!(rendered.contains("intitle:\"Foo Bar\""), "{rendered:?}");
        assert!(!rendered.contains("filetype:"), "{rendered:?}");
    }

    /// `apply_academic`'s half of the same bug class: only the words that
    /// name the artifact ("paper", "study", "journal") should imply
    /// `filetype:pdf`. "citation" and "research" name the topic, not the
    /// format, and must not drop non-PDF results.
    #[test]
    fn academic_recipe_only_sets_filetype_pdf_for_words_that_imply_a_document() {
        let (q, fired) = DorkQuery::from_phrases("citation for this claim");
        assert!(fired.contains(&"academic"));
        assert!(q.filetype.is_none(), "citation names the topic, not a pdf artifact");

        let (q, _) = DorkQuery::from_phrases("recent research on climate policy");
        assert!(q.filetype.is_none(), "research names the topic, not a pdf artifact");

        let (q, _) = DorkQuery::from_phrases("paper on climate policy");
        assert_eq!(q.filetype.as_deref(), Some("pdf"));

        let (q, _) = DorkQuery::from_phrases("published in a journal");
        assert_eq!(q.filetype.as_deref(), Some("pdf"));
    }

    /// Recipes are additive: a content recipe (`document`) plus two modifiers
    /// (`onsite`, `exclusion`) compose in one ask rather than one winning.
    #[test]
    fn recipes_compose_document_onsite_and_exclusion() {
        let (q, fired) = DorkQuery::from_phrases("find a pdf on arxiv.org not appendix");
        assert_eq!(q.filetype.as_deref(), Some("pdf"));
        assert_eq!(q.sites, vec!["arxiv.org".to_string()]);
        assert_eq!(q.exclude, vec!["appendix".to_string()]);
        assert!(fired.contains(&"document"), "{fired:?}");
        assert!(fired.contains(&"onsite"), "{fired:?}");
        assert!(fired.contains(&"exclusion"), "{fired:?}");
    }
}
