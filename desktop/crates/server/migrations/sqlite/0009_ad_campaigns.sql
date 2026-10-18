-- Social advertisement campaigns: the Studio's Ads tab (ADR 0017).
--
-- Two additions, both deliberately small:
--
-- `project.brand_json` is the per-project brand brief — voice, product,
-- audience, links. It is a third blob column beside `workspace_payload_json`
-- and `planning_prefs_json` rather than a key inside one of them, because two
-- writers sharing one JSON object is how a key space collides. Multiple
-- projects to advertise is therefore free: one brief per project row.
--
-- `ad_campaigns` is one row per "make me N ads for this platform". `copy_json`
-- holds the variants the copy pass produced — caption, hashtags, CTA, the
-- image prompt, and the `media_jobs.id` each one started. That reference is a
-- soft FK on purpose: `media_jobs` is capped and pruned, and a campaign whose
-- picture has aged out is still a campaign worth reading, so the variant
-- renders without its image rather than the row disappearing.
--
-- No `workspace_id`: like `media_jobs` this is master-key surface (ADR 0009,
-- "Tenancy"), and `ads.rs` checks project access with `projects::assert_access`
-- so a campaign cannot be filed against someone else's project.
--
-- House rules as ever: TEXT timestamps (`wire::sql_now`), no BOOLEAN column —
-- the `Any` pool refuses to decode one (`db.rs`).

ALTER TABLE project ADD COLUMN brand_json TEXT;

CREATE TABLE IF NOT EXISTS ad_campaigns (
	id INTEGER NOT NULL,
	project_id INTEGER NOT NULL REFERENCES project (id),
	team_template_id INTEGER REFERENCES teamtemplate (id),
	platform VARCHAR(32) NOT NULL,
	brief TEXT NOT NULL,
	status VARCHAR(16) NOT NULL DEFAULT 'running',
	error TEXT,
	copy_json TEXT,
	user_id INTEGER REFERENCES users (id),
	created_at TEXT NOT NULL,
	updated_at TEXT NOT NULL,
	PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS ix_ad_campaigns_project_id ON ad_campaigns (project_id);
CREATE INDEX IF NOT EXISTS ix_ad_campaigns_user_id ON ad_campaigns (user_id);
