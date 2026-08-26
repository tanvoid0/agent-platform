-- Loopback redirect for native magic-link (ADR 0013). The desktop binds
-- 127.0.0.1, sends that URI with the email request, and the verify GET 302s
-- there with the session. Stored on the row so the email clicker cannot
-- swap in a different callback.
--
-- TEXT, nullable: browser /accounts login has no redirect.

ALTER TABLE magic_links ADD COLUMN redirect_uri TEXT;
