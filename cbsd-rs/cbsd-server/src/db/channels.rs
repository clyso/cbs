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

//! Database operations for channels and channel types.
//!
//! All CRUD functions are used by `routes::channels` (commit 4) and
//! the channel resolution helper (commit 5).

use serde::Serialize;
use sqlx::SqlitePool;

/// A channel record as stored in the database.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelRecord {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub default_type_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A channel type record as stored in the database.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelTypeRecord {
    pub id: i64,
    pub channel_id: i64,
    pub type_name: String,
    pub project: String,
    pub prefix_template: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Result of resolving a channel/type pair for build submission.
pub struct ResolvedChannelType {
    pub channel_id: i64,
    pub channel_type_id: i64,
    pub project: String,
    pub prefix_template: String,
}

// ---------------------------------------------------------------------------
// Channel CRUD
// ---------------------------------------------------------------------------

/// Create a new channel. Returns the auto-generated ID.
pub async fn create_channel(
    pool: &SqlitePool,
    name: &str,
    description: &str,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO channels (name, description)
         VALUES (?, ?)
         RETURNING id AS "id!""#,
        name,
        description,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

/// Get a channel by ID (active only).
pub async fn get_channel_by_id(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<ChannelRecord>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id AS "id!", name AS "name!", description AS "description!",
                  default_type_id, created_at AS "created_at!", updated_at AS "updated_at!"
         FROM channels WHERE id = ? AND deleted_at IS NULL"#,
        id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ChannelRecord {
        id: r.id,
        name: r.name,
        description: r.description,
        default_type_id: r.default_type_id,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// Get a channel by name (active only).
pub async fn get_channel_by_name(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<ChannelRecord>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id AS "id!", name AS "name!", description AS "description!",
                  default_type_id, created_at AS "created_at!", updated_at AS "updated_at!"
         FROM channels WHERE name = ? AND deleted_at IS NULL"#,
        name,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ChannelRecord {
        id: r.id,
        name: r.name,
        description: r.description,
        default_type_id: r.default_type_id,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// List all active channels.
pub async fn list_active_channels(
    pool: &SqlitePool,
) -> Result<Vec<ChannelRecord>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", name AS "name!", description AS "description!",
                  default_type_id, created_at AS "created_at!", updated_at AS "updated_at!"
         FROM channels WHERE deleted_at IS NULL ORDER BY name"#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ChannelRecord {
            id: r.id,
            name: r.name,
            description: r.description,
            default_type_id: r.default_type_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Update a channel's name and/or description. Returns true if updated.
pub async fn update_channel(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    description: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE channels SET name = ?, description = ?, updated_at = unixepoch()
         WHERE id = ? AND deleted_at IS NULL",
        name,
        description,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Soft-delete a channel and all its active child types in one transaction.
pub async fn soft_delete_channel(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let result = sqlx::query!(
        "UPDATE channels SET deleted_at = unixepoch(), updated_at = unixepoch()
         WHERE id = ? AND deleted_at IS NULL",
        id,
    )
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() > 0 {
        // Cascade: soft-delete all active child types.
        sqlx::query(
            "UPDATE channel_types SET deleted_at = unixepoch(), updated_at = unixepoch()
             WHERE channel_id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(true)
    } else {
        tx.commit().await?;
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Channel Type CRUD
// ---------------------------------------------------------------------------

/// Create a new channel type. Returns the auto-generated ID.
pub async fn create_type(
    pool: &SqlitePool,
    channel_id: i64,
    type_name: &str,
    project: &str,
    prefix_template: &str,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO channel_types (channel_id, type_name, project, prefix_template)
         VALUES (?, ?, ?, ?)
         RETURNING id AS "id!""#,
        channel_id,
        type_name,
        project,
        prefix_template,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

/// Get a channel type by ID (active only).
pub async fn get_type(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<ChannelTypeRecord>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id AS "id!", channel_id AS "channel_id!", type_name AS "type_name!",
                  project AS "project!", prefix_template AS "prefix_template!",
                  created_at AS "created_at!", updated_at AS "updated_at!"
         FROM channel_types WHERE id = ? AND deleted_at IS NULL"#,
        id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ChannelTypeRecord {
        id: r.id,
        channel_id: r.channel_id,
        type_name: r.type_name,
        project: r.project,
        prefix_template: r.prefix_template,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// List active types for a channel.
pub async fn list_types_for_channel(
    pool: &SqlitePool,
    channel_id: i64,
) -> Result<Vec<ChannelTypeRecord>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", channel_id AS "channel_id!", type_name AS "type_name!",
                  project AS "project!", prefix_template AS "prefix_template!",
                  created_at AS "created_at!", updated_at AS "updated_at!"
         FROM channel_types WHERE channel_id = ? AND deleted_at IS NULL
         ORDER BY type_name"#,
        channel_id,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ChannelTypeRecord {
            id: r.id,
            channel_id: r.channel_id,
            type_name: r.type_name,
            project: r.project,
            prefix_template: r.prefix_template,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Update a channel type's project and prefix_template. Returns true if updated.
pub async fn update_type(
    pool: &SqlitePool,
    id: i64,
    project: &str,
    prefix_template: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE channel_types SET project = ?, prefix_template = ?, updated_at = unixepoch()
         WHERE id = ? AND deleted_at IS NULL",
        project,
        prefix_template,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Soft-delete a channel type. If it was the channel's default_type_id,
/// clears that reference. Both operations run in a single transaction.
pub async fn soft_delete_type(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let result = sqlx::query!(
        "UPDATE channel_types SET deleted_at = unixepoch(), updated_at = unixepoch()
         WHERE id = ? AND deleted_at IS NULL",
        id,
    )
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() > 0 {
        // Clear default_type_id if it pointed to this type.
        sqlx::query!(
            "UPDATE channels SET default_type_id = NULL, updated_at = unixepoch()
             WHERE default_type_id = ?",
            id,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(true)
    } else {
        tx.commit().await?;
        Ok(false)
    }
}

/// Set the default type for a channel. The type must belong to this channel.
pub async fn set_default_type(
    pool: &SqlitePool,
    channel_id: i64,
    type_id: i64,
) -> Result<bool, sqlx::Error> {
    // Only set default_type_id if the type belongs to this channel.
    let result = sqlx::query(
        "UPDATE channels SET default_type_id = ?, updated_at = unixepoch()
         WHERE id = ? AND deleted_at IS NULL
           AND EXISTS (
               SELECT 1 FROM channel_types
               WHERE id = ? AND channel_id = ? AND deleted_at IS NULL
           )",
    )
    .bind(type_id)
    .bind(channel_id)
    .bind(type_id)
    .bind(channel_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Resolve a channel/type pair by name. Returns the IDs, project,
/// and prefix template for a given (channel_name, type_name) combination.
pub async fn resolve_channel_type(
    pool: &SqlitePool,
    channel_name: &str,
    type_name: &str,
) -> Result<Option<ResolvedChannelType>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT ct.id AS "type_id!", ct.channel_id AS "channel_id!",
                  ct.project AS "project!", ct.prefix_template AS "prefix_template!"
         FROM channel_types ct
         JOIN channels c ON c.id = ct.channel_id
         WHERE c.name = ? AND c.deleted_at IS NULL
           AND ct.type_name = ? AND ct.deleted_at IS NULL"#,
        channel_name,
        type_name,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ResolvedChannelType {
        channel_id: r.channel_id,
        channel_type_id: r.type_id,
        project: r.project,
        prefix_template: r.prefix_template,
    }))
}
