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

//! Database operations for roles, capabilities, and user-role assignments.

use sqlx::SqlitePool;

/// A role as stored in the database.
pub struct RoleRecord {
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub created_at: i64,
}

/// A role assignment with optional per-assignment scopes.
#[derive(Debug, Clone)]
pub struct RoleAssignment {
    pub role: String,
    pub scopes: Vec<ScopeEntry>,
}

/// A single scope entry (type + glob pattern).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScopeEntry {
    pub scope_type: String,
    pub pattern: String,
}

/// A user's role with its per-assignment scopes (for listing).
pub struct UserRoleWithScopes {
    pub role_name: String,
    pub scopes: Vec<ScopeEntry>,
}

/// Full assignment details: role name, its capabilities, and per-assignment scopes.
#[allow(dead_code)]
pub struct AssignmentWithScopes {
    pub role_name: String,
    pub caps: Vec<String>,
    pub scopes: Vec<ScopeEntry>,
}

/// Create a new role.
pub async fn create_role(
    pool: &SqlitePool,
    name: &str,
    description: &str,
    builtin: bool,
) -> Result<(), sqlx::Error> {
    let builtin_int = builtin as i32;
    sqlx::query!(
        "INSERT INTO roles (name, description, builtin) VALUES (?, ?, ?)",
        name,
        description,
        builtin_int,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Get a single role by name.
pub async fn get_role(pool: &SqlitePool, name: &str) -> Result<Option<RoleRecord>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT name AS "name!", description AS "description!",
                  builtin, created_at AS "created_at!"
           FROM roles WHERE name = ?"#,
        name,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| RoleRecord {
        name: r.name,
        description: r.description,
        builtin: r.builtin != 0,
        created_at: r.created_at,
    }))
}

/// List all roles.
pub async fn list_roles(pool: &SqlitePool) -> Result<Vec<RoleRecord>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT name AS "name!", description AS "description!",
                  builtin, created_at AS "created_at!"
           FROM roles ORDER BY name"#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RoleRecord {
            name: r.name,
            description: r.description,
            builtin: r.builtin != 0,
            created_at: r.created_at,
        })
        .collect())
}

/// Delete a role by name. Returns `false` if the role is builtin (not deleted).
pub async fn delete_role(pool: &SqlitePool, name: &str) -> Result<bool, sqlx::Error> {
    // Refuse to delete builtin roles
    if is_role_builtin(pool, name).await? {
        return Ok(false);
    }

    let result = sqlx::query!("DELETE FROM roles WHERE name = ? AND builtin = 0", name,)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Replace the capabilities of a role: DELETE all existing + INSERT batch.
pub async fn set_role_caps(
    pool: &SqlitePool,
    role_name: &str,
    caps: &[&str],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    sqlx::query!("DELETE FROM role_caps WHERE role_name = ?", role_name)
        .execute(&mut *tx)
        .await?;

    for cap in caps {
        sqlx::query!(
            "INSERT INTO role_caps (role_name, cap) VALUES (?, ?)",
            role_name,
            *cap,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Get all capabilities for a role.
pub async fn get_role_caps(pool: &SqlitePool, role_name: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT cap AS "cap!" FROM role_caps WHERE role_name = ? ORDER BY cap"#,
        role_name,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| r.cap).collect())
}

/// Add a single role to a user. Ignores conflicts (idempotent).
pub async fn add_user_role(
    pool: &SqlitePool,
    user_email: &str,
    role_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT OR IGNORE INTO user_roles (user_email, role_name) VALUES (?, ?)",
        user_email,
        role_name,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a single role from a user. Returns `true` if a row was removed.
pub async fn remove_user_role(
    pool: &SqlitePool,
    user_email: &str,
    role_name: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM user_roles WHERE user_email = ? AND role_name = ?",
        user_email,
        role_name,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Replace all role assignments (with scopes) for a user in a single transaction.
/// Deletes existing assignments then inserts new ones.
pub async fn set_user_roles(
    pool: &SqlitePool,
    user_email: &str,
    assignments: &[RoleAssignment],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    // Delete existing scopes and role assignments (scopes cascade from user_roles)
    sqlx::query!("DELETE FROM user_roles WHERE user_email = ?", user_email)
        .execute(&mut *tx)
        .await?;

    for assignment in assignments {
        let role = &assignment.role;
        sqlx::query!(
            "INSERT INTO user_roles (user_email, role_name) VALUES (?, ?)",
            user_email,
            role,
        )
        .execute(&mut *tx)
        .await?;

        for scope in &assignment.scopes {
            let scope_type = &scope.scope_type;
            let pattern = &scope.pattern;
            sqlx::query!(
                "INSERT INTO user_role_scopes (user_email, role_name, scope_type, pattern)
                 VALUES (?, ?, ?, ?)",
                user_email,
                role,
                scope_type,
                pattern,
            )
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

/// Get all role assignments for a user, including per-assignment scopes.
pub async fn get_user_roles(
    pool: &SqlitePool,
    user_email: &str,
) -> Result<Vec<UserRoleWithScopes>, sqlx::Error> {
    let role_rows = sqlx::query!(
        r#"SELECT role_name AS "role_name!" FROM user_roles WHERE user_email = ? ORDER BY role_name"#,
        user_email,
    )
    .fetch_all(pool)
    .await?;

    let mut result = Vec::with_capacity(role_rows.len());

    for role_row in role_rows {
        let role_name = role_row.role_name;

        let scope_rows = sqlx::query!(
            r#"SELECT scope_type AS "scope_type!", pattern AS "pattern!"
               FROM user_role_scopes
               WHERE user_email = ? AND role_name = ?
               ORDER BY scope_type, pattern"#,
            user_email,
            role_name,
        )
        .fetch_all(pool)
        .await?;

        let scopes = scope_rows
            .into_iter()
            .map(|r| ScopeEntry {
                scope_type: r.scope_type,
                pattern: r.pattern,
            })
            .collect();

        result.push(UserRoleWithScopes { role_name, scopes });
    }

    Ok(result)
}

/// Get the deduplicated effective capabilities for a user (union across all
/// assigned roles).
pub async fn get_effective_caps(
    pool: &SqlitePool,
    user_email: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT DISTINCT rc.cap AS "cap!"
           FROM user_roles ur
           JOIN role_caps rc ON ur.role_name = rc.role_name
           WHERE ur.user_email = ?
           ORDER BY rc.cap"#,
        user_email,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| r.cap).collect())
}

/// Get all assignments for a user with each assignment's role capabilities and scopes.
pub async fn get_user_assignments_with_scopes(
    pool: &SqlitePool,
    user_email: &str,
) -> Result<Vec<AssignmentWithScopes>, sqlx::Error> {
    let role_rows = sqlx::query!(
        r#"SELECT role_name AS "role_name!" FROM user_roles WHERE user_email = ? ORDER BY role_name"#,
        user_email,
    )
    .fetch_all(pool)
    .await?;

    let mut result = Vec::with_capacity(role_rows.len());

    for role_row in role_rows {
        let role_name = role_row.role_name;

        let cap_rows = sqlx::query!(
            r#"SELECT cap AS "cap!" FROM role_caps WHERE role_name = ? ORDER BY cap"#,
            role_name,
        )
        .fetch_all(pool)
        .await?;

        let caps: Vec<String> = cap_rows.into_iter().map(|r| r.cap).collect();

        let scope_rows = sqlx::query!(
            r#"SELECT scope_type AS "scope_type!", pattern AS "pattern!"
               FROM user_role_scopes
               WHERE user_email = ? AND role_name = ?
               ORDER BY scope_type, pattern"#,
            user_email,
            role_name,
        )
        .fetch_all(pool)
        .await?;

        let scopes = scope_rows
            .into_iter()
            .map(|r| ScopeEntry {
                scope_type: r.scope_type,
                pattern: r.pattern,
            })
            .collect();

        result.push(AssignmentWithScopes {
            role_name,
            caps,
            scopes,
        });
    }

    Ok(result)
}

/// Count active users who hold a role with the `*` capability.
/// Used for the last-admin guard.
pub async fn count_active_wildcard_holders(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT COUNT(DISTINCT u.email) AS "cnt!"
           FROM users u
           JOIN user_roles ur ON u.email = ur.user_email
           JOIN role_caps rc ON ur.role_name = rc.role_name
           WHERE u.active = 1 AND rc.cap = '*'"#,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.cnt.into())
}

/// Check if any user is assigned to a given role.
pub async fn has_assignments(pool: &SqlitePool, role_name: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT EXISTS(SELECT 1 FROM user_roles WHERE role_name = ?) AS "has_any!""#,
        role_name,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.has_any != 0)
}

/// Check if a role is builtin.
pub async fn is_role_builtin(pool: &SqlitePool, role_name: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!("SELECT builtin FROM roles WHERE name = ?", role_name,)
        .fetch_optional(pool)
        .await?;

    Ok(row.is_some_and(|r| r.builtin != 0))
}
