-- The web search module's persisted history (ADR 0008,
-- docs/web-search-module-plan.md's "Saved searches in the database" — no
-- longer deferred). One row per dork the app either *built* (opened = 0) or
-- actually *ran* (opened = 1); `search.rs` promotes the former into the
-- latter on request rather than inserting a near-duplicate.
--
-- `workspace_id` is nullable, same as `project`'s — a master-key caller has
-- none. Tenancy is enforced in `search.rs`, not here: a workspace token's
-- reads/writes are scoped to its own `workspace_id`, and a row belonging to
-- another workspace 404s rather than 401s (see `projects::assert_access`).
--
-- `opened` is `INTEGER`, not `BOOLEAN` — the `Any` pool refuses to decode a
-- `bool` column on either backend (`db.rs`), so the wire is text-then-cast
-- everywhere and the row struct reads this into an `i64`, rendered back to a
-- JSON boolean by `wire::sql_flag`.
--
-- `created_at` is `TEXT`, like every other timestamp in this server —
-- `db.rs::ensure_schema`'s note and `wire::sql_now` are why.
CREATE TABLE IF NOT EXISTS search_history (
	id INTEGER NOT NULL,
	workspace_id INTEGER,
	query VARCHAR NOT NULL,
	engine VARCHAR(32) NOT NULL,
	source VARCHAR(32) NOT NULL,
	opened INTEGER NOT NULL DEFAULT 0,
	created_at TEXT NOT NULL,
	PRIMARY KEY (id),
	CONSTRAINT fk_search_history_workspace_id_workspace FOREIGN KEY(workspace_id) REFERENCES workspace (id)
);

CREATE INDEX IF NOT EXISTS ix_search_history_workspace_id_created_at ON search_history (workspace_id, created_at);
