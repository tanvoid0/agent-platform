-- The SQLite half of `postgres/0008_timestamp_columns_to_text.sql`, which has
-- nothing to do here.
--
-- That migration converts `TIMESTAMP` columns left by the Alembic schema to
-- TEXT, so a drifted Postgres database matches the decision `0001_initial.sql`
-- encodes: every timestamp in this server is a string. SQLite has no timestamp
-- type — its own `DATETIME` is text with a declared affinity, which is what
-- `sql_now()` has always written and `CAST(… AS TEXT)` has always read back.
-- There is no drift to repair.
--
-- The file exists because the two directories are the same schema under the
-- same `_sqlx_migrations` version: a version present in one and missing from
-- the other is what the rule in `0001_initial.sql`'s header ("a schema change
-- is a new `000N_*.sql` in *both* directories") exists to prevent. A statement
-- here would be the bug, not the empty file.

SELECT 1;
