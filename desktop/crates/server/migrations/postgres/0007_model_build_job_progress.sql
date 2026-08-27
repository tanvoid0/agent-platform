-- Postgres form of `sqlite/0007_model_build_job_progress.sql` — see that file.

ALTER TABLE model_build_jobs ADD COLUMN progress_json TEXT;
