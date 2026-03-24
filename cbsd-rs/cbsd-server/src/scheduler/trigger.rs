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
//! interpolates the tag format, and submits a build through the same path as
//! the REST handler (DB insert + in-memory queue + dispatch attempt).

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
/// insert a build record, enqueue it, and attempt dispatch.
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

    // 2. Look up user name/email.
    let user = db::users::get_user(&state.pool, &task.created_by)
        .await
        .map_err(|e| TriggerError::Transient(format!("failed to get user: {e}")))?
        .ok_or(TriggerError::UserDeactivated)?;

    // 3. Parse descriptor JSON.
    let mut descriptor: BuildDescriptor = serde_json::from_str(&task.descriptor)
        .map_err(|e| TriggerError::Fatal(format!("invalid descriptor JSON: {e}")))?;

    // 4. Set signed_off_by from the looked-up user.
    descriptor.signed_off_by.user = user.name;
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

    // 8. Parse priority, defaulting to Normal on unknown values.
    let priority = match task.priority.as_str() {
        "high" => Priority::High,
        "low" => Priority::Low,
        _ => Priority::Normal,
    };

    // 9. Submit build via the shared internal function.
    // Channel resolution not yet wired up for periodic builds — passes None.
    // Commit 6 adds full resolution with scope re-validation.
    let (build_id, _) = crate::routes::builds::insert_build_internal(
        state,
        descriptor,
        &user.email,
        priority,
        Some(&task.id),
        None,
        None,
    )
    .await
    .map_err(TriggerError::Transient)?;

    Ok(build_id)
}
