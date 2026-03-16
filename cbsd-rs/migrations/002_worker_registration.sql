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

-- Worker registration: persistent worker identity bound to API keys.
--
-- Each worker has a server-assigned UUID, a unique human-readable name,
-- and a dedicated API key (one-to-one). Workers are registered via the
-- REST API and authenticate over WebSocket using their bound key.
--
-- Note: after this migration, builds.worker_id stores the registered
-- worker UUID instead of the self-reported display label. Old build
-- records retain their original display labels and will not join to
-- workers.id.

CREATE TABLE IF NOT EXISTS workers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    arch        TEXT NOT NULL CHECK (arch IN ('x86_64', 'aarch64')),
    api_key_id  INTEGER NOT NULL UNIQUE
                REFERENCES api_keys(id) ON DELETE CASCADE,
    created_by  TEXT NOT NULL REFERENCES users(email),
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    last_seen   INTEGER
);
