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
use std::path::PathBuf;

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
/// comes up with them ON and deleting a board that still has items 500s.
///
/// **The schema declares foreign keys the data does not satisfy.** SQLAlchemy
/// left the pragma at SQLite's default OFF, so no row here was ever checked:
/// `PRAGMA foreign_key_check` on a real user database returns 55 violations,
/// every one an `eventlog.task_id` pointing at a tasknode a finished DAG
/// deleted. Turning them on is not a flag flip — it needs a migration that
/// rebuilds the affected tables with `ON DELETE` actions (SQLite has no
/// `ALTER TABLE … ADD CONSTRAINT`) and clears the orphans, and the handlers
/// already delete children explicitly, which is why nothing is broken today.
/// ponytail: worth doing the first time a dangling row causes a visible bug.
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

/// Bring the database up to head, applied at startup.
///
/// **This replaces Alembic.** ADR 0007 rule 2 made Alembic the only migration
/// owner for as long as two servers shared the database — a second migration
/// tool would have raced it. There is no second server now, and Alembic cannot
/// stay: it is `app/alembic/`, which went with the rest of the Python package.
///
/// What ships instead is `crates/server/migrations/`, run by `sqlx::migrate!`,
/// which is embedded in the binary at compile time — the deployed artifact is
/// still one file with no directory to ship beside it.
///
/// **The first migration is a squash, not a replay.** `0001_initial.sql` is the
/// final Alembic head (`e0f1a2b3c4d5`) written as `CREATE TABLE IF NOT EXISTS`
/// plus its indexes; the thirty historical revisions are not carried, because
/// every database in existence was already at head the day Python was deleted.
/// So it is a no-op against an existing database and a one-pass create against
/// an empty one, and either way sqlx records it in `_sqlx_migrations` and never
/// runs it again.
///
/// **A schema change is now a new file**, `000N_what_it_does.sql`, and nothing
/// else. Do not edit an applied one: sqlx stores each file's checksum and
/// refuses to start against a modified copy, which is the guarantee that makes
/// this a migration runner rather than the create-only bootstrap it replaced.
///
/// ponytail: forward-only, no `down` scripts. Rolling back means writing the
/// next migration — worth adding reversibility the first time a release
/// actually has to be undone, not before.
pub async fn ensure_schema(pool: &AnyPool) -> Result<(), sqlx::Error> {
    sqlx::migrate!("./migrations").run(pool).await.map_err(sqlx::Error::from)
}

/// How many timestamped copies [`backup`] keeps.
const BACKUP_GENERATIONS: usize = 3;

/// Take a consistent copy of the database, and prune the oldest.
///
/// **There was no backup of any kind.** The desktop's SQLite file is the whole
/// of a user's projects, plans, threads and workspaces, on a laptop, with
/// nothing copying it anywhere — the failure this closes is not exotic, it is
/// one bad shutdown.
///
/// `VACUUM INTO` rather than a file copy: it is SQLite's own snapshot, it reads
/// through a transaction so a live server writing underneath it cannot produce a
/// torn file, and it defragments on the way out. Copying `*.db` while the WAL
/// holds four megabytes of uncheckpointed pages produces a file that is missing
/// them.
///
/// A failure is logged and swallowed. This runs after the listener is bound, on
/// its own task, and a server that refuses to serve because it could not write a
/// backup is worse than one running without today's copy.
///
/// ponytail: keeps three generations beside the database, so it survives a
/// corrupt file but not a lost disk. Off-machine is a deployment's job, not
/// this process's.
pub async fn backup(pool: &AnyPool, db_path: &std::path::Path) {
    if crate::env_opt("AGENT_PLATFORM_BACKUP").as_deref() == Some("0") {
        return;
    }
    let Some(dir) = db_path.parent() else { return };
    let stem = db_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stamp = crate::request_id::iso_now().replace([':', '-'], "").replace('.', "");
    let target = dir.join(format!("{stem}.{}.bak", &stamp[..stamp.len().min(15)]));

    // The path goes into the statement as a literal — `VACUUM INTO` takes no
    // bind parameter. Single quotes are doubled, which is the only escape SQL
    // string literals have, and the path is ours rather than a caller's.
    let literal = target.display().to_string().replace('\'', "''");
    if let Err(e) = sqlx::query(&format!("VACUUM INTO '{literal}'")).execute(pool).await {
        logd!("backup failed: {e}");
        return;
    }
    logd!("backup written to {}", target.display());
    prune_backups(dir, &stem);
}

/// Keep the newest [`BACKUP_GENERATIONS`], delete the rest.
///
/// Sorted by name, not by mtime: the timestamp is in the name and is fixed
/// width, so it sorts chronologically, and a file whose mtime was changed by a
/// sync client does not reorder the set.
fn prune_backups(dir: &std::path::Path, stem: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut backups: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{stem}.")) && n.ends_with(".bak"))
        })
        .collect();
    if backups.len() <= BACKUP_GENERATIONS {
        return;
    }
    backups.sort();
    for old in &backups[..backups.len() - BACKUP_GENERATIONS] {
        if let Err(e) = std::fs::remove_file(old) {
            logd!("could not remove old backup {}: {e}", old.display());
        }
    }
}

/// Fold the write-ahead log back into the database file and truncate it.
///
/// SQLite checkpoints automatically at 1000 pages but never *truncates*, so the
/// `-wal` sidecar only grows: on a real install it was 4 MB beside a 496 KB
/// database. That is not a correctness problem — it is read on open — but it is
/// the difference between copying one small file and one large one, and it is a
/// single statement at the only moment nothing is writing.
pub async fn checkpoint(pool: &AnyPool) {
    if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(pool).await {
        logd!("wal checkpoint failed: {e}");
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// The whole file has to execute against a real SQLite, twice.
    ///
    /// Once because a broken statement is a server that starts and then 500s on
    /// the first query; twice because every install after the first runs this
    /// against a database that already has the tables, and an `IF NOT EXISTS`
    /// that was missed would fail there and nowhere else.
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

    /// A backup has to be readable, and the set has to stop growing.
    #[tokio::test]
    async fn a_backup_is_a_usable_database_and_only_three_are_kept() {
        let dir = std::env::temp_dir().join(format!("agp-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_platform.db");
        let pool = connect_lazy(&url_for(&path, None), Backend::Sqlite);
        ensure_schema(&pool).await.unwrap();
        sqlx::query("INSERT INTO project (id, name, created_at, updated_at) \
                     VALUES (1, 'kept', '2026-01-01', '2026-01-01')")
            .execute(&pool)
            .await
            .unwrap();

        // Five runs, so the prune has something to do. The stamp has
        // second resolution, so the names are forced apart rather than
        // sleeping through five seconds of test time.
        for n in 0..5 {
            backup(&pool, &path).await;
            let made = newest(&dir);
            std::fs::rename(&made, dir.join(format!("agent_platform.db.2026010100000{n}.bak")))
                .unwrap();
        }
        prune_backups(&dir, "agent_platform.db");

        let mut kept: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".bak"))
            .collect();
        kept.sort();
        assert_eq!(kept.len(), BACKUP_GENERATIONS, "{kept:?}");
        assert!(kept[0].ends_with("000002.bak"), "the oldest go, not the newest: {kept:?}");

        // The copy is a database, not a file of the right size.
        let copy = connect_lazy(&url_for(&dir.join(&kept[2]), None), Backend::Sqlite);
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM project WHERE id = 1")
            .fetch_optional(&copy)
            .await
            .unwrap();
        assert_eq!(name.as_deref(), Some("kept"));

        copy.close().await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn newest(dir: &std::path::Path) -> PathBuf {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "bak"))
            .collect();
        found.sort();
        found.pop().expect("a backup was written")
    }

    /// Every database in the field was built by Alembic and has no
    /// `_sqlx_migrations` table, so the runner sees an unmigrated database that
    /// already holds every table `0001_initial.sql` creates.
    ///
    /// **This is the one way the switch from create-only to a migration runner
    /// could destroy a user's data**, and it fails silently in the good
    /// direction only because the squash is `IF NOT EXISTS` throughout. A
    /// `DROP`/`CREATE` slipped into that file would take the user's rows with
    /// it, and no other test in this crate would notice.
    #[tokio::test]
    async fn an_alembic_built_database_is_adopted_without_losing_rows() {
        let path =
            std::env::temp_dir().join(format!("agp-adopt-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = url_for(&path, None);
        let pool = connect_lazy(&url, Backend::Sqlite);

        // Stand in for what Alembic left behind: the table, with a row in it,
        // and nothing recording that a migration ever ran. `workspace_id` is
        // here because `ix_project_workspace_id` indexes it and `CREATE INDEX
        // IF NOT EXISTS` still resolves the column — a stub without it fails
        // the whole migration, which is what an existing database missing a
        // column would also do.
        sqlx::query(
            "CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT, workspace_id INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO project (id, name) VALUES (1, 'existing')")
            .execute(&pool)
            .await
            .unwrap();

        ensure_schema(&pool).await.expect("adopting an existing database");

        let name: Option<String> = sqlx::query_scalar("SELECT name FROM project WHERE id = 1")
            .fetch_optional(&pool)
            .await
            .unwrap();
        assert_eq!(name.as_deref(), Some("existing"), "the runner dropped a populated table");

        // And the tables the old database did *not* have were created anyway.
        let found: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'api_tokens'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(found.as_deref(), Some("api_tokens"));

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
