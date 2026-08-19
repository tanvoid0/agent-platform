-- Local media generation jobs (ADR 0009). One row per image or video the
-- media module asked ComfyUI to produce — the job outlives the HTTP request
-- that started it, because diffusion runs seconds-to-minutes and the desktop
-- polls rather than holds a response open.
--
-- No `workspace_id`: media jobs are master-key resources like the rest of the
-- desktop's own surface (ADR 0009, "Tenancy"), and `media.rs` requires the
-- master key on every route.
--
-- `status` is queued | running | completed | failed. `comfy_prompt_id` is
-- ComfyUI's own id for the submitted graph; `file_name` is the finished
-- output's name inside the server's `media/` data dir, set only on completion.
--
-- Integer flags/timestamps follow the house rules: `created_at`/`updated_at`
-- are TEXT (`wire::sql_now`), and there is no BOOLEAN column because the `Any`
-- pool refuses to decode one (`db.rs`).
CREATE TABLE IF NOT EXISTS media_jobs (
	id INTEGER NOT NULL,
	kind VARCHAR(16) NOT NULL,
	prompt TEXT NOT NULL,
	enhanced_prompt TEXT,
	status VARCHAR(16) NOT NULL DEFAULT 'queued',
	error TEXT,
	width INTEGER NOT NULL,
	height INTEGER NOT NULL,
	length INTEGER NOT NULL DEFAULT 0,
	seed BIGINT NOT NULL,
	comfy_prompt_id VARCHAR(64),
	file_name VARCHAR(255),
	created_at TEXT NOT NULL,
	updated_at TEXT NOT NULL,
	PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS ix_media_jobs_created_at ON media_jobs (created_at);
