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

//! First-startup database seeding: builtin roles, admin user, worker API keys.

use sqlx::{Row, SqlitePool};

use crate::auth::api_keys;
use crate::config::ServerConfig;

/// Seed the database on first startup if the roles table is empty.
///
/// In a single transaction:
/// 1. Create builtin roles (admin, builder, viewer) with predefined caps.
/// 2. If `seed_admin` is configured, create the admin user and assign the
///    admin role.
/// 3. For each `seed_worker_api_keys` entry, generate an API key owned by
///    the seed admin.
///
/// Plaintext API keys are printed to stdout AFTER the transaction commits.
pub async fn run_first_startup_seed(pool: &SqlitePool, config: &ServerConfig) -> Result<(), SeedError> {
    // Check if the roles table already has data.
    let count: i64 = sqlx::query("SELECT COUNT(*) as cnt FROM roles")
        .fetch_one(pool)
        .await
        .map_err(SeedError::Db)?
        .get("cnt");

    if count > 0 {
        tracing::debug!("roles table is not empty — skipping first-startup seed");
        return Ok(());
    }

    tracing::info!("roles table is empty — running first-startup seed");

    // Collect API key plaintexts to print AFTER commit.
    let mut api_key_plaintexts: Vec<(String, String)> = Vec::new();

    let mut tx = pool.begin().await.map_err(SeedError::Db)?;

    // 1. Create builtin roles.
    create_builtin_role(
        &mut tx,
        "admin",
        "Full administrative access",
        &["*"],
    )
    .await?;

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
        ],
    )
    .await?;

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
        sqlx::query(
            "INSERT INTO users (email, name) VALUES (?, 'Admin')",
        )
        .bind(admin_email)
        .execute(&mut *tx)
        .await
        .map_err(SeedError::Db)?;

        sqlx::query(
            "INSERT INTO user_roles (user_email, role_name) VALUES (?, 'admin')",
        )
        .bind(admin_email)
        .execute(&mut *tx)
        .await
        .map_err(SeedError::Db)?;

        tracing::info!(email = %admin_email, "created seed admin user with admin role");

        // 3. Create worker API keys owned by the seed admin.
        for key_cfg in &config.seed.seed_worker_api_keys {
            let (plaintext, prefix) =
                generate_api_key_in_tx(&mut tx, &key_cfg.name, admin_email).await?;

            tracing::info!(
                name = %key_cfg.name,
                prefix = %prefix,
                "created seed worker API key"
            );

            api_key_plaintexts.push((key_cfg.name.clone(), plaintext));
        }
    } else if !config.seed.seed_worker_api_keys.is_empty() {
        tracing::warn!(
            "seed_worker_api_keys configured but no seed_admin — \
             skipping worker API key creation"
        );
    }

    // Commit the transaction.
    tx.commit().await.map_err(SeedError::Db)?;

    // Print plaintext API keys to stdout AFTER successful commit.
    for (name, plaintext) in &api_key_plaintexts {
        // Use println! (not tracing) — this is intentional: the plaintext
        // must be captured by the operator from stdout and never appears in
        // structured logs.
        println!("Worker API key for {name}: {plaintext}");
    }

    Ok(())
}

/// Create a builtin role with the given capabilities inside a transaction.
async fn create_builtin_role(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    description: &str,
    caps: &[&str],
) -> Result<(), SeedError> {
    sqlx::query(
        "INSERT INTO roles (name, description, builtin) VALUES (?, ?, 1)",
    )
    .bind(name)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(SeedError::Db)?;

    for cap in caps {
        sqlx::query(
            "INSERT INTO role_caps (role_name, cap) VALUES (?, ?)",
        )
        .bind(name)
        .bind(*cap)
        .execute(&mut **tx)
        .await
        .map_err(SeedError::Db)?;
    }

    Ok(())
}

/// Generate an API key inside a transaction. Returns `(plaintext, prefix)`.
///
/// Uses the same key format as `auth::api_keys::create_api_key` but operates
/// within the seed transaction rather than the pool directly.
async fn generate_api_key_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    owner_email: &str,
) -> Result<(String, String), SeedError> {
    use argon2::password_hash::SaltString;
    use argon2::{Argon2, PasswordHasher};
    use rand::Rng;

    // Generate 32 random bytes -> 64 hex chars.
    let random_bytes: [u8; 32] = rand::thread_rng().r#gen();
    let hex_part = api_keys::hex_encode_bytes(&random_bytes);
    let raw_key = format!("cbsk_{hex_part}");

    // Prefix = first 12 hex chars (chars 5..17 of the raw key).
    let prefix = raw_key[5..17].to_string();

    // Argon2 hash (expensive — run in blocking thread).
    let key_clone = raw_key.clone();
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

    sqlx::query(
        "INSERT INTO api_keys (name, key_hash, key_prefix, owner_email) VALUES (?, ?, ?, ?)",
    )
    .bind(name)
    .bind(&hash)
    .bind(&prefix)
    .bind(owner_email)
    .execute(&mut **tx)
    .await
    .map_err(SeedError::Db)?;

    Ok((raw_key, prefix))
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
