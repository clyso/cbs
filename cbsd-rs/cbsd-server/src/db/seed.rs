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

//! First-startup database seeding: builtin roles, admin user, dev workers.

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use sqlx::SqlitePool;

use crate::config::ServerConfig;
use crate::db;

/// Seed the database on first startup if the roles table is empty.
///
/// In a single transaction:
/// 1. Create builtin roles (admin, builder, viewer) with predefined caps.
/// 2. If `seed_admin` is configured, create the admin user and assign the
///    admin role.
/// 3. If `dev.enabled`, seed workers with pre-configured API keys.
pub async fn run_first_startup_seed(
    pool: &SqlitePool,
    config: &ServerConfig,
) -> Result<(), SeedError> {
    let count = sqlx::query!(r#"SELECT COUNT(*) AS "cnt!" FROM roles"#)
        .fetch_one(pool)
        .await
        .map_err(SeedError::Db)?
        .cnt;

    if count > 0 {
        tracing::debug!("roles table is not empty — skipping first-startup seed");
        return Ok(());
    }

    tracing::info!("roles table is empty — running first-startup seed");

    if config.dev.enabled {
        tracing::warn!(
            "DEVELOPMENT MODE — seeding workers with pre-configured API keys. \
             Do not use dev mode in production."
        );
    }

    // Pre-hash dev worker API keys BEFORE opening the transaction (argon2 is
    // CPU-bound and must not hold a pool connection).
    struct PreparedWorker {
        name: String,
        arch: String,
        worker_uuid: String,
        key_prefix: String,
        key_hash: String,
    }

    let mut prepared_workers = Vec::new();
    if config.dev.enabled {
        for worker_cfg in &config.dev.seed_workers {
            let key_clone = worker_cfg.api_key.clone();
            let hash = tokio::task::spawn_blocking(move || {
                let salt = SaltString::generate(&mut rand::thread_rng());
                let argon2 = Argon2::default();
                argon2
                    .hash_password(key_clone.as_bytes(), &salt)
                    .map(|h| h.to_string())
                    .map_err(|e| SeedError::Hash(e.to_string()))
            })
            .await
            .map_err(|e| SeedError::Hash(e.to_string()))??;

            let prefix = worker_cfg.api_key[5..17].to_string();

            prepared_workers.push(PreparedWorker {
                name: worker_cfg.name.clone(),
                arch: worker_cfg.arch.to_string(),
                worker_uuid: uuid::Uuid::new_v4().to_string(),
                key_prefix: prefix,
                key_hash: hash,
            });
        }
    }

    let mut tx = pool.begin().await.map_err(SeedError::Db)?;

    // 1. Create builtin roles.
    create_builtin_role(&mut tx, "admin", "Full administrative access", &["*"]).await?;

    create_builtin_role(
        &mut tx,
        "builder",
        "Create and manage own builds",
        &[
            "builds:create",
            "builds:revoke:own",
            "builds:list:own",
            "builds:list:any",
            "apikeys:create:own",
            "workers:view",
            "channels:view",
        ],
    )
    .await?;

    // Builder gets global scopes so existing behaviour is preserved.
    db::roles::set_role_scopes_in_tx(
        &mut tx,
        "builder",
        &[
            db::roles::ScopeEntry {
                scope_type: "channel".to_string(),
                pattern: "*".to_string(),
            },
            db::roles::ScopeEntry {
                scope_type: "repository".to_string(),
                pattern: "*".to_string(),
            },
        ],
    )
    .await
    .map_err(SeedError::Db)?;

    create_builtin_role(
        &mut tx,
        "viewer",
        "Read-only access to builds and workers",
        &["builds:list:any", "workers:view"],
    )
    .await?;

    tracing::info!("created builtin roles: admin, builder, viewer");

    // 2. Create seed admin user if configured.
    if let Some(admin_email) = &config.seed.seed_admin {
        sqlx::query!(
            "INSERT INTO users (email, name) VALUES (?, 'Admin')",
            admin_email,
        )
        .execute(&mut *tx)
        .await
        .map_err(SeedError::Db)?;

        sqlx::query!(
            "INSERT INTO user_roles (user_email, role_name) VALUES (?, 'admin')",
            admin_email,
        )
        .execute(&mut *tx)
        .await
        .map_err(SeedError::Db)?;

        tracing::info!(email = %admin_email, "created seed admin user with admin role");

        // 3. Seed dev workers with pre-configured API keys.
        for pw in &prepared_workers {
            let api_key_name = format!("worker:{}", pw.name);
            let api_key_id = db::api_keys::insert_api_key_in_tx(
                &mut tx,
                &api_key_name,
                admin_email,
                &pw.key_hash,
                &pw.key_prefix,
            )
            .await
            .map_err(SeedError::Db)?;

            db::workers::insert_worker(
                &mut tx,
                &pw.worker_uuid,
                &pw.name,
                &pw.arch,
                api_key_id,
                admin_email,
            )
            .await
            .map_err(SeedError::Db)?;

            tracing::info!(
                name = %pw.name,
                worker_id = %pw.worker_uuid,
                arch = %pw.arch,
                "seeded dev worker (pre-configured API key)"
            );
        }
    } else if !prepared_workers.is_empty() {
        tracing::warn!("dev.seed_workers configured but no seed_admin — skipping worker creation");
    }

    tx.commit().await.map_err(SeedError::Db)?;

    Ok(())
}

/// Create a builtin role with the given capabilities inside a transaction.
async fn create_builtin_role(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    description: &str,
    caps: &[&str],
) -> Result<(), SeedError> {
    sqlx::query!(
        "INSERT INTO roles (name, description, builtin) VALUES (?, ?, 1)",
        name,
        description,
    )
    .execute(&mut **tx)
    .await
    .map_err(SeedError::Db)?;

    for cap in caps {
        sqlx::query!(
            "INSERT INTO role_caps (role_name, cap) VALUES (?, ?)",
            name,
            *cap,
        )
        .execute(&mut **tx)
        .await
        .map_err(SeedError::Db)?;
    }

    Ok(())
}

/// Errors that can occur during first-startup seeding.
#[derive(Debug)]
pub enum SeedError {
    /// Database error.
    Db(sqlx::Error),
    /// Argon2 hashing error.
    Hash(String),
}

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Hash(e) => write!(f, "hashing error: {e}"),
        }
    }
}

impl std::error::Error for SeedError {}
