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

//! Build trigger logic for periodic tasks.
//!
//! When a periodic task fires, [`trigger_periodic_build`] validates the owner,
//! interpolates the tag format, resolves the channel/type mapping (including
//! scope re-validation), and submits a build through the same path as the
//! REST handler (DB insert + in-memory queue + dispatch attempt).

use crate::app::AppState;
use crate::db;
use crate::db::periodic::PeriodicTaskRow;
use crate::scheduler::tag_format;

use cbsd_proto::{BuildDescriptor, Priority};

/// Errors that can occur when triggering a periodic build.
pub enum TriggerError {
    /// The task owner is deactivated or not found.
    UserDeactivated,
    /// A transient error (e.g. database) that may succeed on retry.
    Transient(String),
    /// A permanent error (e.g. invalid descriptor) — task should be disabled.
    Fatal(String),
}

impl std::fmt::Display for TriggerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserDeactivated => write!(f, "task owner is deactivated or not found"),
            Self::Transient(msg) => write!(f, "transient error: {msg}"),
            Self::Fatal(msg) => write!(f, "fatal error: {msg}"),
        }
    }
}

/// Trigger a periodic build: validate the owner, interpolate the tag,
/// resolve channel/type mapping, and submit a build.
///
/// Channel/type resolution happens at each trigger, not at task creation
/// time. If the channel is renamed or deleted, the trigger fails. If the
/// task owner's scope was revoked, the trigger also fails. This prevents
/// stale permissions from producing builds.
///
/// Returns the new build ID on success.
pub async fn trigger_periodic_build(
    state: &AppState,
    task: &PeriodicTaskRow,
) -> Result<i64, TriggerError> {
    // 1. Check that the task owner is active.
    let active = db::users::is_user_active(&state.pool, &task.created_by)
        .await
        .map_err(|e| TriggerError::Transient(format!("failed to check user active: {e}")))?;

    if !active {
        return Err(TriggerError::UserDeactivated);
    }

    // 2. Look up user record (needed for channel resolution and signed_off_by).
    let user = db::users::get_user(&state.pool, &task.created_by)
        .await
        .map_err(|e| TriggerError::Transient(format!("failed to get user: {e}")))?
        .ok_or(TriggerError::UserDeactivated)?;

    // 3. Parse descriptor JSON.
    let mut descriptor: BuildDescriptor = serde_json::from_str(&task.descriptor)
        .map_err(|e| TriggerError::Fatal(format!("invalid descriptor JSON: {e}")))?;

    // 4. Set signed_off_by from the looked-up user.
    descriptor.signed_off_by.user = user.name.clone();
    descriptor.signed_off_by.email = user.email.clone();

    // 5. Interpolate the tag format.
    let now = chrono::Utc::now();
    let interpolated_tag = tag_format::interpolate_tag(&task.tag_format, &descriptor, now);

    // 6. Validate the interpolated tag as a valid OCI tag.
    tag_format::validate_oci_tag(&interpolated_tag).map_err(|e| {
        TriggerError::Fatal(format!(
            "interpolated tag '{}' is not a valid OCI tag: {e}",
            interpolated_tag
        ))
    })?;

    // 7. Set the destination image tag.
    descriptor.dst_image.tag = interpolated_tag;

    // 8. Resolve channel/type mapping and rewrite dst_image.name.
    //    Re-validates the task owner's scope at trigger time. If the
    //    owner's channel/type scope was revoked since task creation,
    //    this returns an error which feeds into the retry/disable flow.
    let resolved =
        crate::channels::resolve_and_rewrite(&state.pool, &mut descriptor, &user)
            .await
            .map_err(|e| classify_resolution_error(e))?;

    // 9. Parse priority, defaulting to Normal on unknown values.
    let priority = match task.priority.as_str() {
        "high" => Priority::High,
        "low" => Priority::Low,
        _ => Priority::Normal,
    };

    // 10. Submit build via the shared internal function.
    let (build_id, _) = crate::routes::builds::insert_build_internal(
        state,
        descriptor,
        &user.email,
        priority,
        Some(&task.id),
        Some(resolved.channel_id),
        Some(resolved.channel_type_id),
    )
    .await
    .map_err(TriggerError::Transient)?;

    Ok(build_id)
}

/// Classify a channel/type resolution error as Fatal or Transient.
///
/// Permanent failures (scope revoked, channel/type deleted or missing
/// configuration) are Fatal — the task should be disabled immediately
/// instead of burning through retries. DB errors ("failed to") are
/// Transient because they may succeed on retry.
fn classify_resolution_error(msg: String) -> TriggerError {
    const FATAL_PATTERNS: &[&str] = &[
        "insufficient scope",
        "not found",
        "not configured",
        "no default",
    ];

    if FATAL_PATTERNS.iter().any(|p| msg.contains(p)) {
        TriggerError::Fatal(msg)
    } else {
        TriggerError::Transient(msg)
    }
}
