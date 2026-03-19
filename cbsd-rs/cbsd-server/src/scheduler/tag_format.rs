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

//! Tag format interpolation and validation for periodic builds.
//!
//! A tag format string uses `{variable}` placeholders that are expanded
//! at trigger time. Example: `v{version}-nightly.{Y}{m}{d}`

use cbsd_proto::BuildDescriptor;
use chrono::{DateTime, Datelike, Timelike, Utc};

/// Known placeholder names that can appear inside `{...}` in a tag format.
const KNOWN_PLACEHOLDERS: &[&str] = &[
    "Y",
    "m",
    "d",
    "H",
    "M",
    "S",
    "DT",
    "version",
    "base_tag",
    "channel",
    "user",
    "arch",
    "distro",
    "os_version",
];

/// Validate a tag format string. Returns `Ok(())` if all placeholders are
/// known, or `Err` with a list of unrecognized placeholder names.
pub fn validate_tag_format(format: &str) -> Result<(), Vec<String>> {
    let mut unknown = Vec::new();
    let mut chars = format.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            // Collect the placeholder name until '}'.
            let mut name = String::new();
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                name.push(inner);
            }
            if !name.is_empty() && !KNOWN_PLACEHOLDERS.contains(&name.as_str()) {
                unknown.push(name);
            }
        }
    }

    if unknown.is_empty() {
        Ok(())
    } else {
        Err(unknown)
    }
}

/// Interpolate all `{variable}` placeholders in a tag format string.
///
/// Unknown placeholders are left as-is (including the braces).
pub fn interpolate_tag(format: &str, descriptor: &BuildDescriptor, now: DateTime<Utc>) -> String {
    let mut result = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut name = String::new();
            let mut found_close = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    found_close = true;
                    break;
                }
                name.push(inner);
            }

            if !found_close {
                // Unclosed brace: emit literally.
                result.push('{');
                result.push_str(&name);
            } else if let Some(value) = resolve_placeholder(&name, descriptor, now) {
                result.push_str(&value);
            } else {
                // Unknown placeholder: preserve original.
                result.push('{');
                result.push_str(&name);
                result.push('}');
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Resolve a single placeholder name to its value.
fn resolve_placeholder(
    name: &str,
    descriptor: &BuildDescriptor,
    now: DateTime<Utc>,
) -> Option<String> {
    match name {
        "Y" => Some(format!("{:04}", now.year())),
        "m" => Some(format!("{:02}", now.month())),
        "d" => Some(format!("{:02}", now.day())),
        "H" => Some(format!("{:02}", now.hour())),
        "M" => Some(format!("{:02}", now.minute())),
        "S" => Some(format!("{:02}", now.second())),
        "DT" => Some(format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        )),
        "version" => Some(descriptor.version.clone()),
        "base_tag" => Some(descriptor.dst_image.tag.clone()),
        "channel" => Some(descriptor.channel.clone()),
        "user" => Some(descriptor.signed_off_by.user.clone()),
        "arch" => Some(descriptor.build.arch.to_string()),
        "distro" => Some(descriptor.build.distro.clone()),
        "os_version" => Some(descriptor.build.os_version.clone()),
        _ => None,
    }
}

/// Validate that a fully interpolated tag conforms to OCI tag constraints:
/// - Maximum 128 characters
/// - Matches `^[a-zA-Z0-9_][a-zA-Z0-9_.-]*$`
pub fn validate_oci_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() {
        return Err("tag must not be empty".to_string());
    }
    if tag.len() > 128 {
        return Err(format!("tag exceeds 128 characters (got {})", tag.len()));
    }

    let mut chars = tag.chars();

    // First character: [a-zA-Z0-9_]
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {}
        Some(c) => {
            return Err(format!("tag must start with [a-zA-Z0-9_], got '{c}'"));
        }
        None => unreachable!(), // already checked empty
    }

    // Remaining characters: [a-zA-Z0-9_.-]
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-') {
            return Err(format!(
                "tag contains invalid character '{c}'; allowed: [a-zA-Z0-9_.-]"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbsd_proto::{
        Arch, BuildComponent, BuildDescriptor, BuildDestImage, BuildSignedOffBy, BuildTarget,
        VersionType,
    };
    use chrono::TimeZone;

    fn sample_descriptor() -> BuildDescriptor {
        BuildDescriptor {
            version: "19.2.3".to_string(),
            channel: "ces-devel".to_string(),
            version_type: VersionType::Dev,
            signed_off_by: BuildSignedOffBy {
                user: "Alice".to_string(),
                email: "alice@clyso.com".to_string(),
            },
            dst_image: BuildDestImage {
                name: "harbor.clyso.com/ces-devel/ceph".to_string(),
                tag: "v19.2.3-dev.1".to_string(),
            },
            components: vec![BuildComponent {
                name: "ceph".to_string(),
                git_ref: "v19.2.3".to_string(),
                repo: Some("https://github.com/clyso/ceph".to_string()),
            }],
            build: BuildTarget {
                distro: "rockylinux".to_string(),
                os_version: "el9".to_string(),
                artifact_type: "rpm".to_string(),
                arch: Arch::X86_64,
            },
        }
    }

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 18, 14, 30, 45).unwrap()
    }

    // --- validate_tag_format ---

    #[test]
    fn validate_all_known_placeholders() {
        let format =
            "{Y}{m}{d}{H}{M}{S}{DT}{version}{base_tag}{channel}{user}{arch}{distro}{os_version}";
        assert!(validate_tag_format(format).is_ok());
    }

    #[test]
    fn validate_no_placeholders() {
        assert!(validate_tag_format("v1.0.0-nightly").is_ok());
    }

    #[test]
    fn validate_unknown_placeholders() {
        let result = validate_tag_format("v{version}-{foo}-{bar}");
        assert!(result.is_err());
        let unknown = result.unwrap_err();
        assert_eq!(unknown, vec!["foo", "bar"]);
    }

    #[test]
    fn validate_mixed_known_and_unknown() {
        let result = validate_tag_format("{version}-{unknown}");
        assert!(result.is_err());
        let unknown = result.unwrap_err();
        assert_eq!(unknown, vec!["unknown"]);
    }

    #[test]
    fn validate_empty_braces() {
        // Empty braces `{}` — empty name is not in KNOWN_PLACEHOLDERS but also
        // is empty, so we skip it.
        assert!(validate_tag_format("test{}value").is_ok());
    }

    // --- interpolate_tag ---

    #[test]
    fn interpolate_date_placeholders() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag("v{version}-nightly.{Y}{m}{d}", &desc, now);
        assert_eq!(result, "v19.2.3-nightly.20260318");
    }

    #[test]
    fn interpolate_time_placeholders() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag("{H}{M}{S}", &desc, now);
        assert_eq!(result, "143045");
    }

    #[test]
    fn interpolate_dt_placeholder() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag("build-{DT}", &desc, now);
        assert_eq!(result, "build-20260318T143045");
    }

    #[test]
    fn interpolate_descriptor_fields() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag(
            "{channel}-{version}-{arch}-{distro}-{os_version}",
            &desc,
            now,
        );
        assert_eq!(result, "ces-devel-19.2.3-x86_64-rockylinux-el9");
    }

    #[test]
    fn interpolate_base_tag_and_user() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag("{base_tag}-by-{user}", &desc, now);
        assert_eq!(result, "v19.2.3-dev.1-by-Alice");
    }

    #[test]
    fn interpolate_unknown_preserved() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag("v{version}-{unknown}", &desc, now);
        assert_eq!(result, "v19.2.3-{unknown}");
    }

    #[test]
    fn interpolate_no_placeholders() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag("static-tag", &desc, now);
        assert_eq!(result, "static-tag");
    }

    #[test]
    fn interpolate_unclosed_brace() {
        let desc = sample_descriptor();
        let now = fixed_time();
        let result = interpolate_tag("v{version", &desc, now);
        assert_eq!(result, "v{version");
    }

    // --- validate_oci_tag ---

    #[test]
    fn oci_valid_simple() {
        assert!(validate_oci_tag("v19.2.3-nightly.20260318").is_ok());
    }

    #[test]
    fn oci_valid_underscore_start() {
        assert!(validate_oci_tag("_internal").is_ok());
    }

    #[test]
    fn oci_valid_all_allowed_chars() {
        assert!(validate_oci_tag("aZ_09.-test").is_ok());
    }

    #[test]
    fn oci_empty() {
        assert!(validate_oci_tag("").is_err());
    }

    #[test]
    fn oci_too_long() {
        let tag = "a".repeat(129);
        let result = validate_oci_tag(&tag);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("128"));
    }

    #[test]
    fn oci_exactly_128() {
        let tag = "a".repeat(128);
        assert!(validate_oci_tag(&tag).is_ok());
    }

    #[test]
    fn oci_invalid_start_dot() {
        let result = validate_oci_tag(".bad");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must start with"));
    }

    #[test]
    fn oci_invalid_start_dash() {
        let result = validate_oci_tag("-bad");
        assert!(result.is_err());
    }

    #[test]
    fn oci_invalid_char_space() {
        let result = validate_oci_tag("has space");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid character"));
    }

    #[test]
    fn oci_invalid_char_colon() {
        let result = validate_oci_tag("tag:latest");
        assert!(result.is_err());
    }

    #[test]
    fn oci_invalid_char_braces() {
        let result = validate_oci_tag("tag{bad}");
        assert!(result.is_err());
    }
}
