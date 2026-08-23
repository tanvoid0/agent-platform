-- Postgres form of `sqlite/0006_user_ownership.sql` — see that file.

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
