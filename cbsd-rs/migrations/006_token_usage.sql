-- Add first/last usage timestamps to tokens and API keys.
-- NULL = never used. Both columns updated on each successful authentication.
ALTER TABLE tokens ADD COLUMN first_used_at INTEGER;
ALTER TABLE tokens ADD COLUMN last_used_at  INTEGER;

ALTER TABLE api_keys ADD COLUMN first_used_at INTEGER;
ALTER TABLE api_keys ADD COLUMN last_used_at  INTEGER;
