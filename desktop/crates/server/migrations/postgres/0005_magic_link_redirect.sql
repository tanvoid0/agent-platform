-- Postgres form of `sqlite/0005_magic_link_redirect.sql` — see that file.

ALTER TABLE magic_links ADD COLUMN redirect_uri TEXT;
