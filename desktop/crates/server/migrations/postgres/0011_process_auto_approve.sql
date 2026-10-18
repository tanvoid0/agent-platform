-- Auto-approve becomes a property of the process, not of the request that
-- started it.
--
-- It used to live only in the `spawn_plan` argument, so it was readable exactly
-- once — at the plan gate — and could not be changed on a process already
-- running. Persisted here it also gates task review: a `requires_review` task
-- on an auto-approving process completes instead of parking in
-- `awaiting_review`, and `PATCH /processes/{id}` can flip it mid-run.
--
-- INTEGER 0/1 on both backends, like `tasknode.requires_review`.
-- Forward-only and defaulted, so an in-flight process survives the upgrade.

ALTER TABLE process ADD COLUMN auto_approve INTEGER NOT NULL DEFAULT 0;
