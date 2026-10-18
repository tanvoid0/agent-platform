-- A task node can produce a picture, not only text (ADR 0018).
--
-- `modality` is `text` (the default, and what every existing row is), `image`
-- or `video`. The executor dispatches on it: `text` goes to the chat proxy as
-- it always has, the other two go to `media::start_job` — the same row, waiter
-- and file route Studio and the ads campaigns already use.
--
-- `media_job_id` is the job that node started, so a reader does not have to
-- parse it back out of `output`. Soft FK for the same reason `ad_campaigns`
-- keeps one: `media_jobs` is capped and pruned, and a finished process whose
-- picture has aged out is still worth reading.
--
-- Forward-only and defaulted, so an in-flight process survives the upgrade.

ALTER TABLE tasknode ADD COLUMN modality TEXT NOT NULL DEFAULT 'text';
ALTER TABLE tasknode ADD COLUMN media_job_id INTEGER;
