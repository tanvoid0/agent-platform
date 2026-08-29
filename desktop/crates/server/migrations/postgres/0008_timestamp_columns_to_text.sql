-- Bring a drifted Postgres database back to the "every timestamp is TEXT"
-- decision `0001_initial.sql` encodes.
--
-- **The bug this repairs.** `wire::sql_now()` produces a string, every INSERT
-- binds a string, and Postgres refuses to put one in a `timestamp` column:
--
--     column "created_at" is of type timestamp without time zone
--     but expression is of type text
--
-- A database created by these migrations never sees that — `0001_initial.sql`
-- declares TEXT. A database created by the *Alembic* schema this server
-- replaced declares `TIMESTAMP`, because SQLAlchemy's `DateTime` maps to one,
-- and every write to such a column has 500ed ever since the Rust server took
-- over. `POST /api/v1/teams` is where it was found; it is not special, it is
-- just the first one someone called.
--
-- **Why convert the column rather than cast at the call site.** The decision in
-- `0001_initial.sql` is that a timestamp is a string end to end: `sql_now()`
-- writes one, `CAST(… AS TEXT)` reads one back, and the scheduler compares them
-- with `<=`. Casting at the write sites instead would mean several hundred
-- edits to keep a column type nothing reads as a date — and would leave the two
-- backends genuinely different, which is the thing `db.rs` exists to avoid.
--
-- **Safety.** Only the 33 tables this schema declares are touched, by name: a
-- database that also hosts something else keeps its own columns whatever they
-- are. A column already TEXT is not matched, so this is a no-op on a database
-- built from `0001_initial.sql` and safe to run against either. The text it
-- writes is `sql_now()`'s own fixed-width `YYYY-MM-DD HH:MM:SS.ffffff`, which
-- is what `iso_from_sql` parses and what sorts correctly as text.
--
-- A `timestamptz` is converted at UTC, because that is the wall clock every
-- other row in these columns already holds — `sql_now()` is `Utc::now()`.

DO $$
DECLARE
    col RECORD;
    had_default BOOLEAN;
BEGIN
    FOR col IN
        SELECT c.table_name, c.column_name, c.data_type, c.column_default
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON t.table_schema = c.table_schema
         AND t.table_name = c.table_name
        WHERE c.table_schema = current_schema()
          AND t.table_type = 'BASE TABLE'
          AND c.data_type IN ('timestamp without time zone', 'timestamp with time zone')
          AND c.table_name IN (
              'action_sessions', 'action_sets', 'actions', 'api_token_usage_daily',
              'api_tokens', 'assistant_chat_threads', 'assistant_domain_profiles',
              'assistant_reviews', 'coder_chat_threads', 'eventlog', 'magic_links',
              'media_jobs', 'model_build_jobs', 'model_projects',
              'model_registry_entries', 'planner_agent_profiles', 'process', 'project',
              'search_history', 'session_results', 'session_steps', 'sessions',
              'tasknode', 'teamtemplate', 'todo_boards', 'todo_categories',
              'todo_item_events', 'todo_items', 'user_usage_daily', 'users',
              'workflow_runs', 'workflows', 'workspace'
          )
    LOOP
        -- A `DEFAULT CURRENT_TIMESTAMP` cannot be cast to the new type on its
        -- own ("default for column cannot be cast automatically to type text"),
        -- so it comes off first and goes back on afterwards. Re-added as
        -- CURRENT_TIMESTAMP, which a TEXT column takes through the same
        -- assignment cast `0001_initial.sql` relies on.
        had_default := col.column_default IS NOT NULL
                       AND col.column_default ILIKE '%current_timestamp%';
        IF col.column_default IS NOT NULL THEN
            EXECUTE format('ALTER TABLE %I ALTER COLUMN %I DROP DEFAULT',
                           col.table_name, col.column_name);
        END IF;

        IF col.data_type = 'timestamp with time zone' THEN
            EXECUTE format(
                'ALTER TABLE %I ALTER COLUMN %I TYPE TEXT '
                'USING to_char(%I AT TIME ZONE ''UTC'', ''YYYY-MM-DD HH24:MI:SS.US'')',
                col.table_name, col.column_name, col.column_name);
        ELSE
            EXECUTE format(
                'ALTER TABLE %I ALTER COLUMN %I TYPE TEXT '
                'USING to_char(%I, ''YYYY-MM-DD HH24:MI:SS.US'')',
                col.table_name, col.column_name, col.column_name);
        END IF;

        IF had_default THEN
            EXECUTE format('ALTER TABLE %I ALTER COLUMN %I SET DEFAULT CURRENT_TIMESTAMP',
                           col.table_name, col.column_name);
        END IF;
    END LOOP;
END $$;
