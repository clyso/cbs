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

use sqlx::{SqliteConnection, SqlitePool};

/// A role as stored in the database.
pub struct RoleRecord {
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub created_at: i64,
}

/// A single scope entry (type + glob pattern).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScopeEntry {
    pub scope_type: String,
    pub pattern: String,
}

/// A user's role with its role-level scopes (for listing).
pub struct UserRoleWithScopes {
    pub role_name: String,
    pub scopes: Vec<ScopeEntry>,
}

/// Full assignment details: role name, its capabilities, and role-level scopes.
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

/// Replace both capabilities and scopes of a role atomically.
pub async fn set_role_caps_and_scopes(
    pool: &SqlitePool,
    role_name: &str,
    caps: &[&str],
    scopes: &[ScopeEntry],
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

    sqlx::query!("DELETE FROM role_scopes WHERE role_name = ?", role_name)
        .execute(&mut *tx)
        .await?;

    for scope in scopes {
        let scope_type = &scope.scope_type;
        let pattern = &scope.pattern;
        sqlx::query!(
            "INSERT INTO role_scopes (role_name, scope_type, pattern) VALUES (?, ?, ?)",
            role_name,
            scope_type,
            pattern,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Set role scopes inside an existing transaction (used by seed).
pub async fn set_role_scopes_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    role_name: &str,
    scopes: &[ScopeEntry],
) -> Result<(), sqlx::Error> {
    for scope in scopes {
        let scope_type = &scope.scope_type;
        let pattern = &scope.pattern;
        sqlx::query!(
            "INSERT INTO role_scopes (role_name, scope_type, pattern) VALUES (?, ?, ?)",
            role_name,
            scope_type,
            pattern,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Get all scopes for a role.
pub async fn get_role_scopes(
    pool: &SqlitePool,
    role_name: &str,
) -> Result<Vec<ScopeEntry>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT scope_type AS "scope_type!", pattern AS "pattern!"
           FROM role_scopes WHERE role_name = ?
           ORDER BY scope_type, pattern"#,
        role_name,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ScopeEntry {
            scope_type: r.scope_type,
            pattern: r.pattern,
        })
        .collect())
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

/// Replace all role assignments for a user in a single transaction.
/// Scopes are defined on roles, not per-assignment.
pub async fn set_user_roles(
    pool: &SqlitePool,
    user_email: &str,
    role_names: &[&str],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    sqlx::query!("DELETE FROM user_roles WHERE user_email = ?", user_email)
        .execute(&mut *tx)
        .await?;

    for role in role_names {
        sqlx::query!(
            "INSERT INTO user_roles (user_email, role_name) VALUES (?, ?)",
            user_email,
            *role,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Get all role assignments for a user, including role-level scopes.
/// Uses a single JOIN query instead of N+1 round trips.
pub async fn get_user_roles(
    pool: &SqlitePool,
    user_email: &str,
) -> Result<Vec<UserRoleWithScopes>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT ur.role_name AS "role_name!",
                  rs.scope_type AS "scope_type?",
                  rs.pattern AS "pattern?"
           FROM user_roles ur
           LEFT JOIN role_scopes rs ON ur.role_name = rs.role_name
           WHERE ur.user_email = ?
           ORDER BY ur.role_name, rs.scope_type, rs.pattern"#,
        user_email,
    )
    .fetch_all(pool)
    .await?;

    let mut result: Vec<UserRoleWithScopes> = Vec::new();
    for row in rows {
        let scope = match (row.scope_type, row.pattern) {
            (Some(st), Some(p)) => Some(ScopeEntry {
                scope_type: st,
                pattern: p,
            }),
            _ => None,
        };
        if let Some(last) = result.last_mut().filter(|r| r.role_name == row.role_name) {
            if let Some(s) = scope {
                last.scopes.push(s);
            }
        } else {
            result.push(UserRoleWithScopes {
                role_name: row.role_name,
                scopes: scope.into_iter().collect(),
            });
        }
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

/// Get all assignments for a user with each role's capabilities and scopes.
/// Uses two JOIN queries (caps + scopes) instead of 2N+1 round trips.
pub async fn get_user_assignments_with_scopes(
    pool: &SqlitePool,
    user_email: &str,
) -> Result<Vec<AssignmentWithScopes>, sqlx::Error> {
    // Fetch roles + caps in one query
    let cap_rows = sqlx::query!(
        r#"SELECT ur.role_name AS "role_name!",
                  rc.cap AS "cap?"
           FROM user_roles ur
           LEFT JOIN role_caps rc ON ur.role_name = rc.role_name
           WHERE ur.user_email = ?
           ORDER BY ur.role_name, rc.cap"#,
        user_email,
    )
    .fetch_all(pool)
    .await?;

    // Fetch roles + scopes in one query
    let scope_rows = sqlx::query!(
        r#"SELECT ur.role_name AS "role_name!",
                  rs.scope_type AS "scope_type?",
                  rs.pattern AS "pattern?"
           FROM user_roles ur
           LEFT JOIN role_scopes rs ON ur.role_name = rs.role_name
           WHERE ur.user_email = ?
           ORDER BY ur.role_name, rs.scope_type, rs.pattern"#,
        user_email,
    )
    .fetch_all(pool)
    .await?;

    // Build role -> caps map
    let mut caps_map: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for row in &cap_rows {
        let entry = caps_map.entry(row.role_name.clone()).or_default();
        if let Some(cap) = &row.cap
            && !entry.contains(cap)
        {
            entry.push(cap.clone());
        }
    }

    // Build role -> scopes map
    let mut scopes_map: std::collections::BTreeMap<String, Vec<ScopeEntry>> =
        std::collections::BTreeMap::new();
    for row in &scope_rows {
        let entry = scopes_map.entry(row.role_name.clone()).or_default();
        if let (Some(st), Some(p)) = (&row.scope_type, &row.pattern) {
            let scope = ScopeEntry {
                scope_type: st.clone(),
                pattern: p.clone(),
            };
            if !entry
                .iter()
                .any(|e| e.scope_type == scope.scope_type && e.pattern == scope.pattern)
            {
                entry.push(scope);
            }
        }
    }

    // Merge into result (BTreeMap iteration is sorted by key)
    let mut result = Vec::new();
    for (role_name, caps) in caps_map {
        let scopes = scopes_map.remove(&role_name).unwrap_or_default();
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

    Ok(row.cnt)
}

/// Transactional variant of [`count_active_wildcard_holders`].
///
/// Used inside an existing transaction (e.g. in `deactivate_entity`) so that
/// the count reflects the deactivation that was just applied but not yet
/// committed.
pub async fn count_active_wildcard_holders_tx(
    tx: &mut SqliteConnection,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT COUNT(DISTINCT u.email) AS "cnt!"
           FROM users u
           JOIN user_roles ur ON u.email = ur.user_email
           JOIN role_caps rc ON ur.role_name = rc.role_name
           WHERE u.active = 1 AND rc.cap = '*'"#,
    )
    .fetch_one(&mut *tx)
    .await?;

    Ok(row.cnt)
}

/// Check if a role is builtin.
pub async fn is_role_builtin(pool: &SqlitePool, role_name: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!("SELECT builtin FROM roles WHERE name = ?", role_name,)
        .fetch_optional(pool)
        .await?;

    Ok(row.is_some_and(|r| r.builtin != 0))
}
