-- Every tenant row is owned by a `users` row, local and cloud alike.
--
-- Local loopback (ADR 0013) used to have no user: `Principal::unrestricted`
-- with `user_id = NULL`, and tables such as `coder_chat_threads` / `media_jobs`
-- stored nothing to isolate one caller from another. Cloud JWT callers had a
-- user but `workspace_id = NULL`, which the tenancy checks treated as the
-- master key — so one account could read another account's projects.
--
-- `kind` is `local` (OS username, seeded at startup) or `cloud` (magic-link).
-- Same columns, same `/api/v1/me` shape. Orphan tables that never had a
-- workspace (coder, media, workflows, action sets, search history) get
-- `user_id` of their own; everything else hangs off `workspace.user_id`.

ALTER TABLE users ADD COLUMN username VARCHAR(256);
ALTER TABLE users ADD COLUMN kind VARCHAR(16) NOT NULL DEFAULT 'cloud';

ALTER TABLE workspace ADD COLUMN user_id INTEGER REFERENCES users (id);
CREATE INDEX IF NOT EXISTS ix_workspace_user_id ON workspace (user_id);

ALTER TABLE coder_chat_threads ADD COLUMN user_id INTEGER REFERENCES users (id);
CREATE INDEX IF NOT EXISTS ix_coder_chat_threads_user_id ON coder_chat_threads (user_id);

ALTER TABLE media_jobs ADD COLUMN user_id INTEGER REFERENCES users (id);
CREATE INDEX IF NOT EXISTS ix_media_jobs_user_id ON media_jobs (user_id);

ALTER TABLE workflows ADD COLUMN user_id INTEGER REFERENCES users (id);
CREATE INDEX IF NOT EXISTS ix_workflows_user_id ON workflows (user_id);

ALTER TABLE action_sets ADD COLUMN user_id INTEGER REFERENCES users (id);
CREATE INDEX IF NOT EXISTS ix_action_sets_user_id ON action_sets (user_id);

ALTER TABLE search_history ADD COLUMN user_id INTEGER REFERENCES users (id);
CREATE INDEX IF NOT EXISTS ix_search_history_user_id ON search_history (user_id);
