// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

pub mod api_keys;
pub mod builds;
pub mod periodic;
pub mod roles;
pub mod seed;
pub mod tokens;
pub mod users;
pub mod workers;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

/// Create and configure the SQLite connection pool.
///
/// Pragmas set per-connection:
/// - `journal_mode=WAL` — concurrent readers with single writer
/// - `foreign_keys=ON` — FK constraints enforced (SQLite default is OFF)
/// - `busy_timeout=5000` — wait 5s on write contention
/// - `synchronous=NORMAL` — WAL + NORMAL avoids per-checkpoint fsync
///
/// Pool sizing: `max_connections = 4` (correctness requirement — prevents
/// deadlock when the dispatch mutex holds across a SQLite write).
pub async fn create_pool(db_url: &str) -> SqlitePool {
    let options = SqliteConnectOptions::from_str(db_url)
        .expect("invalid database URL")
        .create_if_missing(true)
        .pragma("journal_mode", "WAL")
        .pragma("foreign_keys", "ON")
        .pragma("busy_timeout", "5000")
        .pragma("synchronous", "NORMAL");

    SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .expect("failed to connect to SQLite database")
}

/// Run embedded sqlx migrations.
pub async fn run_migrations(pool: &SqlitePool) {
    sqlx::migrate!("../migrations")
        .run(pool)
        .await
        .expect("failed to run database migrations");
}
