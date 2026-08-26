-- Portal user accounts, sessions, magic links, and per-user usage.
--
-- INTEGER flags, not BOOLEAN: the `Any` pool refuses a bool column (see db.rs).
-- Timestamps are TEXT (`wire::sql_now`), same as every other table here.
--
-- `entitlement` is trial | paid | comp | blocked. One row per email; a second
-- trial is refused by the unique email, not by application logic alone.

CREATE TABLE IF NOT EXISTS users (
	id INTEGER NOT NULL,
	email VARCHAR(320) NOT NULL,
	is_admin INTEGER NOT NULL DEFAULT 0,
	entitlement VARCHAR(16) NOT NULL DEFAULT 'trial',
	trial_ends_at TEXT,
	billing_region VARCHAR(8),
	stripe_customer_id VARCHAR(64),
	stripe_subscription_id VARCHAR(64),
	stripe_price_id VARCHAR(64),
	comp_reason TEXT,
	comp_expires_at TEXT,
	created_at TEXT NOT NULL,
	updated_at TEXT NOT NULL,
	PRIMARY KEY (id)
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_users_email ON users (email);
CREATE INDEX IF NOT EXISTS ix_users_entitlement ON users (entitlement);

CREATE TABLE IF NOT EXISTS sessions (
	id INTEGER NOT NULL,
	user_id INTEGER NOT NULL,
	refresh_token_hash VARCHAR(64) NOT NULL,
	expires_at TEXT NOT NULL,
	revoked INTEGER NOT NULL DEFAULT 0,
	created_at TEXT NOT NULL,
	PRIMARY KEY (id),
	FOREIGN KEY(user_id) REFERENCES users (id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_sessions_refresh_hash ON sessions (refresh_token_hash);
CREATE INDEX IF NOT EXISTS ix_sessions_user_id ON sessions (user_id);

CREATE TABLE IF NOT EXISTS magic_links (
	id INTEGER NOT NULL,
	email VARCHAR(320) NOT NULL,
	token_hash VARCHAR(64) NOT NULL,
	expires_at TEXT NOT NULL,
	used INTEGER NOT NULL DEFAULT 0,
	created_at TEXT NOT NULL,
	PRIMARY KEY (id)
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_magic_links_token_hash ON magic_links (token_hash);
CREATE INDEX IF NOT EXISTS ix_magic_links_email ON magic_links (email);

CREATE TABLE IF NOT EXISTS user_usage_daily (
	user_id INTEGER NOT NULL,
	usage_date VARCHAR(10) NOT NULL,
	request_count INTEGER NOT NULL DEFAULT 0,
	error_count INTEGER NOT NULL DEFAULT 0,
	total_tokens INTEGER NOT NULL DEFAULT 0,
	PRIMARY KEY (user_id, usage_date),
	FOREIGN KEY(user_id) REFERENCES users (id) ON DELETE CASCADE
);
