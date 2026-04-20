-- Extend users table for robot identity.
ALTER TABLE users ADD COLUMN is_robot          INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN robot_description TEXT;

-- Robot bearer-token store.
-- One active (non-revoked) row per robot, enforced by the partial unique index.
-- ON DELETE CASCADE is defensive only: the design forbids hard-deleting a
-- robot row (tombstone preserves users.active=0 for builds.user_email FK
-- integrity). If a future admin path ever deletes a users row directly,
-- CASCADE ensures robot_tokens is cleaned up rather than left orphaned.
CREATE TABLE robot_tokens (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    robot_email   TEXT    NOT NULL
                  REFERENCES users(email) ON DELETE CASCADE,
    token_hash    TEXT    NOT NULL UNIQUE,  -- Argon2id
    token_prefix  TEXT    NOT NULL,         -- first 12 hex chars
    expires_at    INTEGER,                  -- NULL = no expiry
    revoked       INTEGER NOT NULL DEFAULT 0,
    first_used_at INTEGER,
    last_used_at  INTEGER,
    created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_robot_tokens_prefix ON robot_tokens(token_prefix);
CREATE INDEX idx_robot_tokens_robot  ON robot_tokens(robot_email);

-- Guarantees at most one active token per robot; closes concurrent-create race.
CREATE UNIQUE INDEX idx_robot_tokens_active
    ON robot_tokens(robot_email) WHERE revoked = 0;
