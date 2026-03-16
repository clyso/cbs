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

//! Database operations for registered workers.

use sqlx::{Row, SqlitePool};

/// Full worker row from the database.
pub struct WorkerRow {
    pub id: String,
    pub name: String,
    pub arch: String,
    pub api_key_id: i64,
    pub created_by: String,
    pub created_at: i64,
    pub last_seen: Option<i64>,
}

/// Insert a new worker inside an existing transaction.
pub async fn insert_worker(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
    name: &str,
    arch: &str,
    api_key_id: i64,
    created_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO workers (id, name, arch, api_key_id, created_by) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(arch)
    .bind(api_key_id)
    .bind(created_by)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Look up a worker by its UUID.
pub async fn get_worker_by_id(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<WorkerRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, arch, api_key_id, created_by, created_at, last_seen
         FROM workers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| WorkerRow {
        id: r.get("id"),
        name: r.get("name"),
        arch: r.get("arch"),
        api_key_id: r.get("api_key_id"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        last_seen: r.get("last_seen"),
    }))
}

/// Look up a worker by its bound API key row ID.
pub async fn get_worker_by_api_key_id(
    pool: &SqlitePool,
    api_key_id: i64,
) -> Result<Option<WorkerRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, arch, api_key_id, created_by, created_at, last_seen
         FROM workers WHERE api_key_id = ?",
    )
    .bind(api_key_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| WorkerRow {
        id: r.get("id"),
        name: r.get("name"),
        arch: r.get("arch"),
        api_key_id: r.get("api_key_id"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        last_seen: r.get("last_seen"),
    }))
}

/// List all registered workers.
pub async fn list_workers(pool: &SqlitePool) -> Result<Vec<WorkerRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, arch, api_key_id, created_by, created_at, last_seen
         FROM workers ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WorkerRow {
            id: r.get("id"),
            name: r.get("name"),
            arch: r.get("arch"),
            api_key_id: r.get("api_key_id"),
            created_by: r.get("created_by"),
            created_at: r.get("created_at"),
            last_seen: r.get("last_seen"),
        })
        .collect())
}

/// Delete a worker by UUID. Returns true if a row was deleted.
#[allow(dead_code)]
pub async fn delete_worker(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM workers WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Update `last_seen` to the current Unix timestamp. Returns true if
/// a row was updated (false if the worker was deleted mid-flight).
pub async fn update_last_seen(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE workers SET last_seen = unixepoch() WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Update the bound API key inside a transaction (used by token regeneration).
pub async fn update_api_key_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
    new_api_key_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE workers SET api_key_id = ? WHERE id = ?")
        .bind(new_api_key_id)
        .bind(id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}
