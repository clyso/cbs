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

//! Channel/type resolution for build submission.
//!
//! The [`resolve_and_rewrite`] function is the single source of truth for
//! resolving a channel and type from a build descriptor, validating the
//! user's scopes, and rewriting `dst_image.name` to include the project
//! and prefix from the channel/type mapping.
//!
//! Both the REST `submit_build` handler and the periodic scheduler trigger
//! call this function to ensure consistent resolution logic.

use cbsd_proto::BuildDescriptor;
use sqlx::SqlitePool;

use crate::db;
use crate::db::users::UserRecord;

/// Result of channel/type resolution.
pub struct ResolvedChannel {
    pub channel_id: i64,
    pub channel_type_id: i64,
}

/// Resolve channel and type from a build descriptor, validate scopes,
/// and rewrite `dst_image.name` to `<project>/<prefix>/<image>`.
///
/// Resolution order:
/// 1. If `descriptor.channel` is present and non-empty, look up by name.
/// 2. Otherwise, use the user's `default_channel_id`.
/// 3. Resolve type from `descriptor.version_type`, or fall back to the
///    channel's `default_type_id`.
/// 4. Validate the user has a channel scope matching `channel/type`.
/// 5. Resolve prefix template (`${username}` -> email prefix).
/// 6. Rewrite `dst_image.name` to `<project>/<prefix>/<image>`.
pub async fn resolve_and_rewrite(
    pool: &SqlitePool,
    descriptor: &mut BuildDescriptor,
    user: &UserRecord,
) -> Result<ResolvedChannel, String> {
    // 1. Resolve channel.
    let channel_name = descriptor.channel.as_deref().filter(|s| !s.is_empty());

    let channel = if let Some(name) = channel_name {
        db::channels::get_channel_by_name(pool, name)
            .await
            .map_err(|e| format!("failed to look up channel: {e}"))?
            .ok_or_else(|| format!("channel '{}' not found", name))?
    } else {
        // Use user's default channel.
        let channel_id = user.default_channel_id.ok_or(
            "no channel specified and no default channel assigned — contact your administrator"
                .to_string(),
        )?;
        db::channels::get_channel_by_id(pool, channel_id)
            .await
            .map_err(|e| format!("failed to look up default channel: {e}"))?
            .ok_or("default channel no longer exists — contact your administrator".to_string())?
    };

    // 2. Resolve type — explicit from descriptor, or channel default.
    let type_name: String = match descriptor.version_type {
        Some(ref vt) => match vt {
            cbsd_proto::VersionType::Release => "release",
            cbsd_proto::VersionType::Dev => "dev",
            cbsd_proto::VersionType::Test => "test",
            cbsd_proto::VersionType::Ci => "ci",
        }
        .to_string(),
        None => {
            // Use channel's default type.
            let default_type_id = channel.default_type_id.ok_or_else(|| {
                format!(
                    "channel '{}' has no default type — specify --type explicitly",
                    channel.name
                )
            })?;
            db::channels::get_type(pool, default_type_id)
                .await
                .map_err(|e| format!("failed to look up default type: {e}"))?
                .ok_or_else(|| {
                    format!(
                        "channel '{}' default type no longer exists — contact your administrator",
                        channel.name
                    )
                })?
                .type_name
        }
    };

    let resolved = db::channels::resolve_channel_type(pool, &channel.name, &type_name)
        .await
        .map_err(|e| format!("failed to resolve channel/type: {e}"))?
        .ok_or_else(|| {
            format!(
                "type '{}' is not configured for channel '{}'",
                type_name, channel.name
            )
        })?;

    // 3. Validate scope: user must have channel scope for "channel_name/type_name".
    let scope_value = format!("{}/{}", channel.name, type_name);
    let has_scope = check_channel_scope(pool, &user.email, &scope_value).await?;
    if !has_scope {
        return Err(format!(
            "insufficient scope for channel/type '{}'",
            scope_value
        ));
    }

    // 4. Resolve prefix template.
    let prefix = resolve_prefix_template(&resolved.prefix_template, &user.email);

    // 5. Rewrite dst_image.name.
    let original_image = &descriptor.dst_image.name;
    let new_name = if prefix.is_empty() {
        format!("{}/{}", resolved.project, original_image)
    } else {
        format!("{}/{}/{}", resolved.project, prefix, original_image)
    };
    descriptor.dst_image.name = new_name;

    // 6. Set the resolved channel and type in the descriptor for downstream.
    descriptor.channel = Some(channel.name);
    descriptor.version_type = Some(match type_name.as_str() {
        "release" => cbsd_proto::VersionType::Release,
        "dev" => cbsd_proto::VersionType::Dev,
        "test" => cbsd_proto::VersionType::Test,
        "ci" => cbsd_proto::VersionType::Ci,
        _ => cbsd_proto::VersionType::Dev,
    });

    Ok(ResolvedChannel {
        channel_id: resolved.channel_id,
        channel_type_id: resolved.channel_type_id,
    })
}

/// Check whether a user has a channel scope that matches the given value.
///
/// The value is in `channel/type` format. The user's scope patterns
/// are checked: exact match, wildcard suffix, or global `*`.
async fn check_channel_scope(
    pool: &SqlitePool,
    email: &str,
    scope_value: &str,
) -> Result<bool, String> {
    let assignments = db::roles::get_user_assignments_with_scopes(pool, email)
        .await
        .map_err(|e| format!("failed to load user assignments: {e}"))?;

    let ok = assignments.iter().any(|a| {
        // Assignments without scopes (e.g. admin wildcard) pass all checks.
        if a.scopes.is_empty() {
            return true;
        }
        a.scopes.iter().any(|s| {
            s.scope_type == "channel"
                && crate::scopes::scope_pattern_matches(&s.pattern, scope_value)
        })
    });

    Ok(ok)
}

/// Resolve a prefix template by replacing known variables.
///
/// Currently only `${username}` is supported, which resolves to the
/// part of the email address before `@`.
fn resolve_prefix_template(template: &str, email: &str) -> String {
    if template.is_empty() {
        return String::new();
    }

    let username = email.split('@').next().unwrap_or(email);
    template.replace("${username}", username)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_template_username() {
        assert_eq!(
            resolve_prefix_template("${username}", "joao.luis@clyso.com"),
            "joao.luis"
        );
    }

    #[test]
    fn prefix_template_empty() {
        assert_eq!(resolve_prefix_template("", "joao@clyso.com"), "");
    }

    #[test]
    fn prefix_template_no_at() {
        assert_eq!(resolve_prefix_template("${username}", "admin"), "admin");
    }

    #[test]
    fn prefix_template_literal() {
        assert_eq!(
            resolve_prefix_template("static-prefix", "user@clyso.com"),
            "static-prefix"
        );
    }
}
