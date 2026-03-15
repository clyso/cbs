-- Copyright (C) 2026  Clyso
--
-- This program is free software: you can redistribute it and/or modify
-- it under the terms of the GNU Affero General Public License as published by
-- the Free Software Foundation, either version 3 of the License, or
-- (at your option) any later version.
--
-- This program is distributed in the hope that it will be useful,
-- but WITHOUT ANY WARRANTY; without even the implied warranty of
-- MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
-- GNU Affero General Public License for more details.

-- cbsd-rs initial schema
-- All timestamps are INTEGER (Unix epoch seconds).

-- Users: created on first SSO login
CREATE TABLE IF NOT EXISTS users (
    email       TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    active      INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Tokens: PASETO tokens for human users
-- token_hash uses SHA-256 (not argon2)
CREATE TABLE IF NOT EXISTS tokens (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_email  TEXT NOT NULL REFERENCES users(email),
    token_hash  TEXT NOT NULL UNIQUE,
    expires_at  INTEGER,
    revoked     INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_tokens_user ON tokens(user_email);

-- API keys: for service accounts and workers
-- key_hash uses argon2 (offline brute-force resistance)
CREATE TABLE IF NOT EXISTS api_keys (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    key_hash    TEXT NOT NULL UNIQUE,
    key_prefix  TEXT NOT NULL,
    owner_email TEXT NOT NULL REFERENCES users(email),
    expires_at  INTEGER,
    revoked     INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (owner_email, key_prefix),
    UNIQUE (name, owner_email)
);

-- Roles: named permission sets
CREATE TABLE IF NOT EXISTS roles (
    name        TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    builtin     INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Role capabilities
CREATE TABLE IF NOT EXISTS role_caps (
    role_name   TEXT NOT NULL REFERENCES roles(name) ON DELETE CASCADE,
    cap         TEXT NOT NULL,
    PRIMARY KEY (role_name, cap)
);

-- User-role assignments
CREATE TABLE IF NOT EXISTS user_roles (
    user_email  TEXT NOT NULL REFERENCES users(email) ON DELETE CASCADE,
    role_name   TEXT NOT NULL REFERENCES roles(name) ON DELETE CASCADE,
    PRIMARY KEY (user_email, role_name)
);

-- Per-assignment scopes
CREATE TABLE IF NOT EXISTS user_role_scopes (
    user_email  TEXT NOT NULL,
    role_name   TEXT NOT NULL,
    scope_type  TEXT NOT NULL
                CHECK (scope_type IN ('channel', 'registry', 'repository')),
    pattern     TEXT NOT NULL,
    FOREIGN KEY (user_email, role_name)
        REFERENCES user_roles(user_email, role_name) ON DELETE CASCADE,
    UNIQUE (user_email, role_name, scope_type, pattern)
);

-- Builds: persistent record of every build
CREATE TABLE IF NOT EXISTS builds (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    descriptor          TEXT NOT NULL,
    descriptor_version  INTEGER NOT NULL DEFAULT 1,
    user_email          TEXT NOT NULL REFERENCES users(email),
    priority            TEXT NOT NULL DEFAULT 'normal'
                        CHECK (priority IN ('high', 'normal', 'low')),
    state               TEXT NOT NULL DEFAULT 'queued'
                        CHECK (state IN ('queued', 'dispatched', 'started',
                                         'revoking', 'success', 'failure', 'revoked')),
    worker_id           TEXT,
    trace_id            TEXT,
    error               TEXT,
    submitted_at        INTEGER NOT NULL DEFAULT (unixepoch()),
    queued_at           INTEGER NOT NULL DEFAULT (unixepoch()),
    started_at          INTEGER,
    finished_at         INTEGER
);

CREATE INDEX IF NOT EXISTS idx_builds_state ON builds(state);
CREATE INDEX IF NOT EXISTS idx_builds_user ON builds(user_email);
CREATE INDEX IF NOT EXISTS idx_builds_state_queued ON builds(state, queued_at);

-- Build log metadata
CREATE TABLE IF NOT EXISTS build_logs (
    build_id    INTEGER PRIMARY KEY REFERENCES builds(id) ON DELETE CASCADE,
    log_path    TEXT NOT NULL,
    log_size    INTEGER NOT NULL DEFAULT 0,
    finished    INTEGER NOT NULL DEFAULT 0,
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);
