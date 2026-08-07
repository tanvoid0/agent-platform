//! One SQL code path over two backends.
//!
//! `agent-platformd` reads a schema Alembic owns, and that schema exists in both
//! SQLite (the desktop) and Postgres (the cloud deploy, and this repo's own
//! `.env`). Rather than two implementations of every query, the pool is
//! `sqlx::Any` and the differences are handled in exactly two places — here.
//!
//! **The two differences, both measured rather than assumed:**
//!
//! 1. **Placeholders.** SQLite takes `?`, Postgres takes `$1..$n`. [`sql`]
//!    rewrites, skipping anything inside a string literal.
//! 2. **Types the `Any` driver will not decode.** It refuses a timestamp column
//!    on *both* backends (`Any driver does not support the SQLite type
//!    SqliteTypeInfo(Datetime)` / `the Postgres type PgTypeInfo(Timestamp)`),
//!    and a Postgres `integer` is int4 where this code wants `i64`. The fix is
//!    in the SQL, not here: select ids as `CAST(x AS BIGINT)` and timestamps as
//!    `CAST(x AS TEXT)`. Both backends accept that syntax, and on SQLite the
//!    cast is a no-op over text it already stores — the same string comes back.
//!
//! So a query written for this module reads:
//!
//! ```sql
//! SELECT CAST(id AS BIGINT) AS id, name, CAST(created_at AS TEXT) AS created_at
//! FROM project WHERE id = ?
//! ```
//!
//! and runs unchanged on either backend.

use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::AnyPool;
use std::borrow::Cow;

/// Which backend a pool is talking to. Decided once at startup from the URL,
/// because every query needs it and asking the pool per call is noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

impl Backend {
    /// Postgres is opt-in through `DATABASE_URL`; everything else is the
    /// desktop's SQLite file.
    pub fn from_url(url: &str) -> Self {
        let u = url.trim_start().to_ascii_lowercase();
        if u.starts_with("postgres://") || u.starts_with("postgresql://") {
            Backend::Postgres
        } else {
            Backend::Sqlite
        }
    }
}

/// Rewrite `?` placeholders to `$1..$n` for Postgres; leave SQLite alone.
///
/// Quoting matters: a literal `'what?'` in a `LIKE` pattern or a seeded string
/// is not a placeholder, and renumbering it would shift every parameter after
/// it. SQL escapes a quote by doubling it, which this handles for free — the
/// second quote of `''` just toggles back in.
pub fn sql(query: &str, backend: Backend) -> Cow<'_, str> {
    if backend == Backend::Sqlite || !query.contains('?') {
        return Cow::Borrowed(query);
    }
    let mut out = String::with_capacity(query.len() + 8);
    let mut n = 0usize;
    let mut in_string = false;
    for c in query.chars() {
        match c {
            '\'' => {
                in_string = !in_string;
                out.push(c);
            }
            '?' if !in_string => {
                n += 1;
                out.push('$');
                out.push_str(&n.to_string());
            }
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Lazy pool for either backend, with the one per-connection difference applied.
///
/// **SQLite foreign keys must be turned off, and under `Any` a PRAGMA is the
/// only way left to do it.** `SqliteConnectOptions::foreign_keys(false)` is not
/// reachable from an `AnyPool`, and the URL form is rejected outright
/// (`unknown query parameter 'foreign_keys'`) — so without this hook SQLite
/// comes up with them ON, and deleting a board that still has items turns
/// Python's 204 into a 500. The schema declares foreign keys the data does not
/// honour; matching Python is the contract.
///
/// **The hook has to be backend-conditional, and getting that wrong does not
/// fail loudly.** Running the PRAGMA against Postgres makes `after_connect`
/// return `Err`, every connection is discarded on creation, and the pool
/// reports `pool timed out while waiting for an open connection` — a hang, not
/// a syntax error, with nothing pointing at the hook.
pub fn connect_lazy(url: &str, backend: Backend) -> AnyPool {
    // `Any` dispatches on the URL scheme at connect time and panics without
    // this; calling it twice is harmless.
    install_default_drivers();

    let opts = AnyPoolOptions::new();
    let opts = match backend {
        Backend::Sqlite => opts.after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut *conn).await?;
                Ok(())
            })
        }),
        Backend::Postgres => opts,
    };
    // Still lazy: `serve` calls [`ensure_schema`] before it binds, but a test
    // that builds an `AppState` and never touches the database should not need
    // a file on disk for it.
    opts.connect_lazy(url).expect("connection string was validated at startup")
}

/// The schema Alembic used to create, applied at startup.
///
/// **This replaces Alembic, and the replacement is deliberately dumber than it
/// was.** ADR 0007 rule 2 made Alembic the only migration owner for as long as
/// two servers shared the database — a second migration tool would have raced
/// it. There is no second server now, and Alembic cannot stay: it is
/// `app/alembic/`, which went with the rest of the Python package.
///
/// What ships instead is one `schema.sql`, generated from the final Alembic
/// head (`e0f1a2b3c4d5`) as `CREATE TABLE IF NOT EXISTS` plus its indexes. An
/// existing database already has every one of those tables, so applying it is a
/// no-op there; a fresh one gets the schema in a single pass instead of
/// replaying thirty revisions.
///
/// ponytail: **this creates, it does not migrate.** A future column change has
/// nowhere to go — the honest upgrade is a versioned migration runner
/// (`sqlx::migrate!`, or a `schema_version` table and a list of steps), and it
/// should be built the first time a column actually has to change rather than
/// speculatively now. The thirty historical revisions are not worth carrying
/// into it: every database in existence is already at head.
pub async fn ensure_schema(pool: &AnyPool) -> Result<(), sqlx::Error> {
    // Comment lines are dropped **before** splitting, not after. Splitting
    // first glues the file's header comment onto the first `CREATE`, and a
    // filter on "starts with `--`" then silently discards both — which showed
    // up as one missing table, not as an error.
    let sql: String = SCHEMA_SQL
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("
");

    for statement in sql.split(';') {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        sqlx::query(statement).execute(pool).await?;
    }
    Ok(())
}

const SCHEMA_SQL: &str = include_str!("schema.sql");

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// The whole file has to execute against a real SQLite, twice.
    ///
    /// Once because a statement the splitter mangles is a server that starts
    /// and then 500s on the first query; twice because every install after the
    /// first runs this against a database that already has the tables, and an
    /// `IF NOT EXISTS` that was missed would fail there and nowhere else.
    #[tokio::test]
    async fn the_schema_applies_to_an_empty_database_and_again_to_a_full_one() {
        let path = std::env::temp_dir().join(format!("agp-schema-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = url_for(&path, None);
        let pool = connect_lazy(&url, Backend::Sqlite);

        ensure_schema(&pool).await.expect("first apply");
        ensure_schema(&pool).await.expect("second apply must be a no-op");

        // Spot-check the tables the domains actually query, one per area, so a
        // truncated or misparsed file cannot pass this.
        // Real table names, which are not all what the domains are called:
        // teams live in `teamtemplate` and DAG tasks in `tasknode`.
        for table in [
            "project", "teamtemplate", "process", "tasknode", "todo_items", "todo_boards",
            "workflows", "workflow_runs", "api_tokens", "workspace", "assistant_chat_threads",
            "coder_chat_threads", "model_build_jobs", "model_registry_entries", "action_sets",
            "eventlog",
        ] {
            let found: Option<String> = sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
            )
            .bind(table)
            .fetch_optional(&pool)
            .await
            .unwrap();
            assert_eq!(found.as_deref(), Some(table), "missing table {table}");
        }

        // Indexes came across too — they are separate statements in the file
        // and a splitter that dropped them would still pass the check above.
        let indexes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name LIKE 'ix_%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(indexes > 20, "only {indexes} named indexes");

        pool.close().await;
        let _ = std::fs::remove_file(&path);
    }
}

/// SQLite wants a path, `Any` wants a URL. Postgres DSNs pass through untouched.
pub fn url_for(db_path: &std::path::Path, database_url: Option<&str>) -> String {
    match database_url {
        Some(dsn) => dsn.to_string(),
        // Forward slashes: a Windows `\` is an escape inside a URL, and sqlx
        // parses this string as one. `mode=rwc` creates the file — the `Any`
        // driver takes no `create_if_missing`, and this is the pool
        // `ensure_schema` runs on, so without it a fresh install cannot make
        // the database it is about to populate.
        None => format!("sqlite:{}?mode=rwc", db_path.display().to_string().replace('\\', "/")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_is_left_exactly_alone() {
        let q = "SELECT * FROM project WHERE id = ? AND name = ?";
        assert!(matches!(sql(q, Backend::Sqlite), Cow::Borrowed(_)));
        assert_eq!(sql(q, Backend::Sqlite), q);
    }

    #[test]
    fn postgres_numbers_placeholders_in_order() {
        assert_eq!(
            sql("SELECT * FROM project WHERE id = ? AND name = ?", Backend::Postgres),
            "SELECT * FROM project WHERE id = $1 AND name = $2"
        );
        assert_eq!(
            sql("INSERT INTO t (a, b, c) VALUES (?, ?, ?)", Backend::Postgres),
            "INSERT INTO t (a, b, c) VALUES ($1, $2, $3)"
        );
    }

    /// A `?` inside a literal is data. Renumbering it would shift every
    /// parameter after it, and the query would still be valid SQL — so this
    /// fails silently at runtime rather than loudly at compile time.
    #[test]
    fn a_question_mark_inside_a_literal_is_not_a_placeholder() {
        assert_eq!(
            sql("SELECT * FROM t WHERE q = 'what?' AND id = ?", Backend::Postgres),
            "SELECT * FROM t WHERE q = 'what?' AND id = $1"
        );
        // An escaped quote (`''`) toggles in and straight back out.
        assert_eq!(
            sql("SELECT 'it''s a ?' AS a WHERE id = ?", Backend::Postgres),
            "SELECT 'it''s a ?' AS a WHERE id = $1"
        );
    }

    #[test]
    fn a_query_without_placeholders_is_not_copied() {
        let q = "SELECT COUNT(*) FROM project";
        assert!(matches!(sql(q, Backend::Postgres), Cow::Borrowed(_)));
    }

    #[test]
    fn a_sqlite_path_becomes_a_url_and_a_dsn_passes_through() {
        let p = std::path::Path::new(r"C:\Users\x\agent_platform.db");
        assert_eq!(url_for(p, None), "sqlite:C:/Users/x/agent_platform.db?mode=rwc");
        assert_eq!(
            url_for(p, Some("postgresql://u:p@h/db")),
            "postgresql://u:p@h/db"
        );
    }

    /// The FK pragma is the whole reason this constructor exists; running it
    /// against Postgres makes every connection fail to open and the pool hang.
    #[tokio::test]
    async fn sqlite_pools_come_up_with_foreign_keys_off() {
        let dir = std::env::temp_dir().join("agentd-db-fk-test");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("fk.db");
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::write(&file, b"");

        let url = url_for(&file, None);
        let pool = connect_lazy(&url, Backend::Sqlite);
        let on: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .expect("pragma readable");
        assert_eq!(on, 0, "foreign keys must be off, or board deletes 500 where Python returns 204");
    }

    #[test]
    fn backend_is_read_from_the_url_scheme() {
        assert_eq!(Backend::from_url("postgresql://u:p@h/db"), Backend::Postgres);
        assert_eq!(Backend::from_url("postgres://u:p@h/db"), Backend::Postgres);
        assert_eq!(Backend::from_url("  POSTGRES://u@h/db"), Backend::Postgres);
        assert_eq!(Backend::from_url("sqlite:///C:/x/agent_platform.db"), Backend::Sqlite);
        assert_eq!(Backend::from_url("data/agent_platform.db"), Backend::Sqlite);
    }
}
