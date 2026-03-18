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

-- Periodic build tasks: cron-scheduled builds with tag interpolation,
-- retry persistence, and operational visibility.

CREATE TABLE IF NOT EXISTS periodic_tasks (
    id                  TEXT PRIMARY KEY,
    cron_expr           TEXT NOT NULL,
    tag_format          TEXT NOT NULL,
    descriptor          TEXT NOT NULL,
    descriptor_version  INTEGER NOT NULL DEFAULT 1,
    priority            TEXT NOT NULL DEFAULT 'normal'
                        CHECK (priority IN ('high', 'normal', 'low')),
    summary             TEXT,
    enabled             INTEGER NOT NULL DEFAULT 1,
    created_by          TEXT NOT NULL REFERENCES users(email),
    created_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    retry_count         INTEGER NOT NULL DEFAULT 0,
    retry_at            INTEGER,
    last_error          TEXT,
    last_triggered_at   INTEGER,
    last_build_id       INTEGER REFERENCES builds(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_periodic_enabled
    ON periodic_tasks(enabled);

-- Traceability: link builds to the periodic task that spawned them.
ALTER TABLE builds ADD COLUMN periodic_task_id TEXT
    REFERENCES periodic_tasks(id) ON DELETE SET NULL;
