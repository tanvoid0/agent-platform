-- Structured progress for a build job, so a client that connects halfway
-- through a two-hour fine-tune sees a number rather than a blank bar.
--
-- The `train` stage prints `@@AGP:progress@@ {json}` markers (see
-- `worker/model_ops/progress.py`); the runner keeps the newest one here. Only
-- the newest: this is a gauge, not a series. The history is already in the job
-- log, which is where anyone wanting the loss curve should read it from, and a
-- row rewritten every ten steps must stay small.

ALTER TABLE model_build_jobs ADD COLUMN progress_json TEXT;
