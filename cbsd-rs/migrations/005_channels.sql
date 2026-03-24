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

-- Channels: named groupings that represent destination contexts.
-- Soft-deleted via deleted_at; partial unique index ensures name
-- uniqueness among active channels only.
CREATE TABLE IF NOT EXISTS channels (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    default_type_id INTEGER
                    REFERENCES channel_types(id),
    deleted_at      INTEGER,
    created_at      INTEGER NOT NULL
                    DEFAULT (unixepoch()),
    updated_at      INTEGER NOT NULL
                    DEFAULT (unixepoch())
);

CREATE UNIQUE INDEX idx_channels_name_active
    ON channels(name) WHERE deleted_at IS NULL;

-- Channel types: per-channel build classification mapping to a
-- Harbor project and optional prefix template.
-- type_name restricted to the four VersionType enum values.
CREATE TABLE IF NOT EXISTS channel_types (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id       INTEGER NOT NULL
                     REFERENCES channels(id)
                     ON DELETE CASCADE,
    type_name        TEXT NOT NULL
                     CHECK (type_name IN
                       ('dev','release','test','ci')),
    project          TEXT NOT NULL,
    prefix_template  TEXT NOT NULL DEFAULT '',
    deleted_at       INTEGER,
    created_at       INTEGER NOT NULL
                     DEFAULT (unixepoch()),
    updated_at       INTEGER NOT NULL
                     DEFAULT (unixepoch())
);

CREATE UNIQUE INDEX idx_channel_types_active
    ON channel_types(channel_id, type_name)
    WHERE deleted_at IS NULL;

-- Add default channel to users for implicit channel resolution.
ALTER TABLE users ADD COLUMN default_channel_id
    INTEGER REFERENCES channels(id)
    ON DELETE SET NULL;

-- Track which channel/type a build was submitted under.
ALTER TABLE builds ADD COLUMN channel_id
    INTEGER REFERENCES channels(id);
ALTER TABLE builds ADD COLUMN channel_type_id
    INTEGER REFERENCES channel_types(id);
