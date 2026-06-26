// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! Version-descriptor creation (design 006). Source:
//! `cbscore/versions/create.py`. Assembles a [`VersionDescriptor`] from CLI
//! inputs, derives its title, and writes it to the version store.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::DateTime;
use uuid::Uuid;

use crate::components::{ComponentError, CoreComponentLoc, load_components};
use crate::types::store::descriptor_path;
use crate::types::{
    Error, VersionComponent, VersionDescriptor, VersionImage, VersionSignedOffBy, VersionType,
};
use crate::versions::parse::parse_version;
use crate::versions::validate::validate_version;
use crate::versions::version_type::get_version_type_desc;

/// The descriptor-shaping inputs `versions create` collects (the CLI flags plus
/// the git identity). Grouped so [`create`] and [`version_create_helper`] take a
/// single spec rather than a dozen positional arguments. Resolution mechanics
/// (where to find component definitions, and the raw `COMPONENT=URI` overrides)
/// are passed separately — they are not part of the descriptor's shape.
pub struct VersionSpec<'a> {
    pub version: &'a str,
    pub version_type: VersionType,
    pub component_refs: &'a BTreeMap<String, String>,
    pub distro: &'a str,
    pub el_version: u32,
    pub registry: &'a str,
    pub image_name: &'a str,
    pub image_tag: Option<&'a str>,
    pub user_name: &'a str,
    pub user_email: &'a str,
}

/// An error creating or writing a version descriptor.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    /// A version/parse/validation failure (type layer).
    #[error(transparent)]
    Version(#[from] Error),
    /// A component-loading failure.
    #[error(transparent)]
    Component(#[from] ComponentError),
    /// A descriptor already exists at the target path.
    #[error("version for '{version}' already exists at '{path}'")]
    AlreadyExists { version: String, path: Utf8PathBuf },
    /// An IO failure writing the descriptor.
    #[error("error writing descriptor to '{path}'")]
    Write {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Build the title. For an `M.m.p` version this matches Python's
/// `_do_version_title`:
/// `Release <desc> [<PREFIX> ]version <M>.<m>.<p>[ (<suffix-parts>)]`, where each
/// `-`-separated suffix part uppercases its first `.`-segment (`rc.1` → `RC 1`).
/// For a UUIDv7 version it is `Release <desc> version created at <ts>`, where
/// `<ts>` is the ISO-8601 UTC time embedded in the UUIDv7 (design 006).
fn version_title(version: &str, version_type: VersionType) -> Result<String, Error> {
    let desc = get_version_type_desc(version_type);

    if let Ok(parsed) = parse_version(version)
        && let (Some(minor), Some(patch)) = (parsed.minor.as_deref(), parsed.patch.as_deref())
    {
        let mut body = String::new();
        if let Some(prefix) = parsed.prefix.as_deref() {
            body.push_str(&prefix.to_uppercase());
            body.push(' ');
        }
        body.push_str(&format!("version {}.{}.{}", parsed.major, minor, patch));
        if let Some(suffix) = parsed.suffix.as_deref() {
            let parts: Vec<String> = suffix.split('-').map(format_suffix_part).collect();
            let joined = parts.join(", ");
            if !joined.is_empty() {
                body.push_str(&format!(" ({joined})"));
            }
        }
        return Ok(format!("Release {desc} {body}"));
    }

    if let Some(ts) = uuidv7_timestamp(version) {
        return Ok(format!("Release {desc} version created at {ts}"));
    }

    Err(Error::VersionError(format!(
        "malformed version '{version}'"
    )))
}

/// Uppercase a suffix part's first `.`-segment: `rc.1` → `RC 1`, `ga` → `GA`.
fn format_suffix_part(part: &str) -> String {
    match part.split_once('.') {
        Some((head, tail)) => format!("{} {tail}", head.to_uppercase()),
        None => part.to_uppercase(),
    }
}

/// The ISO-8601 UTC (seconds) time embedded in a UUIDv7, or `None` if `version`
/// is not a UUIDv7.
fn uuidv7_timestamp(version: &str) -> Option<String> {
    let uuid = Uuid::parse_str(version).ok()?;
    if uuid.get_version() != Some(uuid::Version::SortRand) {
        return None;
    }
    let (secs, _nanos) = uuid.get_timestamp()?.to_unix();
    let dt = DateTime::from_timestamp(secs as i64, 0)?;
    Some(dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// Assemble a [`VersionDescriptor`] from already-loaded components. A ref's repo
/// URL comes from its component definition, overridden by `component_uri_overrides`;
/// the component must be defined (an override does not bypass that, matching
/// Python). Pure (no IO).
pub fn create(
    spec: &VersionSpec,
    components: &BTreeMap<String, CoreComponentLoc>,
    component_uri_overrides: &BTreeMap<String, String>,
) -> Result<VersionDescriptor, Error> {
    validate_version(spec.version)?;

    if spec.component_refs.is_empty() {
        return Err(Error::VersionError("missing valid components".to_string()));
    }

    let mut components_res = Vec::with_capacity(spec.component_refs.len());
    for (name, git_ref) in spec.component_refs {
        let Some(loc) = components.get(name) else {
            return Err(Error::VersionError(format!(
                "unknown component '{name}' specified"
            )));
        };
        let repo = component_uri_overrides
            .get(name)
            .cloned()
            .unwrap_or_else(|| loc.comp.repo.clone());
        components_res.push(VersionComponent {
            name: name.clone(),
            repo,
            git_ref: git_ref.clone(),
        });
    }

    let title = version_title(spec.version, spec.version_type)?;
    let tag = spec.image_tag.unwrap_or(spec.version).to_string();

    Ok(VersionDescriptor {
        schema_version: 1,
        version: spec.version.to_string(),
        title,
        signed_off_by: VersionSignedOffBy {
            user: spec.user_name.to_string(),
            email: spec.user_email.to_string(),
        },
        image: VersionImage {
            registry: spec.registry.to_string(),
            name: spec.image_name.to_string(),
            tag,
        },
        components: components_res,
        distro: spec.distro.to_string(),
        el_version: spec.el_version,
    })
}

/// Parse `COMPONENT=URI` overrides, load components from `components_paths`
/// (default `<cwd>/components` when empty), then [`create`] the descriptor.
/// Async because it reads the component definitions.
pub async fn version_create_helper(
    spec: &VersionSpec<'_>,
    components_paths: &[Utf8PathBuf],
    component_uri_overrides: &[String],
) -> Result<VersionDescriptor, CreateError> {
    // `COMPONENT=URI` override parsing (design 006 owns it).
    let mut overrides = BTreeMap::new();
    for ov in component_uri_overrides {
        let Some((comp, uri)) = ov.split_once('=') else {
            return Err(Error::VersionError(format!("malformed URI override '{ov}'")).into());
        };
        overrides.insert(comp.to_string(), uri.to_string());
    }

    let resolved_paths = if components_paths.is_empty() {
        let cwd = std::env::current_dir().map_err(|_| {
            Error::VersionError("no components paths provided, nor could any be found!".to_string())
        })?;
        let cwd = Utf8PathBuf::from_path_buf(cwd)
            .map_err(|_| Error::VersionError("current directory is not valid UTF-8".to_string()))?;
        let default = cwd.join("components");
        let is_dir = tokio::fs::metadata(&default)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !is_dir {
            return Err(Error::VersionError(
                "no components paths provided, nor could any be found!".to_string(),
            )
            .into());
        }
        vec![default]
    } else {
        components_paths.to_vec()
    };

    let components = load_components(&resolved_paths).await?;
    if components.is_empty() {
        return Err(Error::VersionError(
            "no components provided, nor could they be found!".to_string(),
        )
        .into());
    }

    Ok(create(spec, &components, &overrides)?)
}

/// Write `desc` to `<store_root>/<type>/<version>.json`, creating parent
/// directories. An existing file at that path is a [`CreateError::AlreadyExists`]
/// guard (matching Python's "version already exists").
pub async fn write_descriptor(
    desc: &VersionDescriptor,
    store_root: &Utf8Path,
    version_type: VersionType,
) -> Result<Utf8PathBuf, CreateError> {
    let path = descriptor_path(store_root, version_type, &desc.version);

    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(CreateError::AlreadyExists {
            version: desc.version.clone(),
            path,
        });
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| CreateError::Write {
                path: path.clone(),
                source,
            })?;
    }
    let json = desc.to_json_pretty().map_err(|e| CreateError::Write {
        path: path.clone(),
        source: std::io::Error::other(e),
    })?;
    tokio::fs::write(&path, json)
        .await
        .map_err(|source| CreateError::Write {
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_component() -> BTreeMap<String, CoreComponentLoc> {
        use crate::components::{CoreComponent, CoreComponentBuild, CoreComponentContainers};
        let mut m = BTreeMap::new();
        m.insert(
            "ceph".to_string(),
            CoreComponentLoc {
                path: Utf8PathBuf::from("/components/ceph"),
                comp: CoreComponent {
                    name: "ceph".to_string(),
                    repo: "https://github.com/ceph/ceph".to_string(),
                    build: CoreComponentBuild {
                        rpm: None,
                        get_version: "get_version.sh".to_string(),
                        deps: "install_deps.sh".to_string(),
                    },
                    containers: CoreComponentContainers {
                        path: "containers".into(),
                    },
                },
            },
        );
        m
    }

    fn refs() -> BTreeMap<String, String> {
        BTreeMap::from([("ceph".to_string(), "v20.2.1".to_string())])
    }

    /// A `dev`-typed spec over the given version and refs, with stub identity.
    fn dev_spec<'a>(version: &'a str, refs: &'a BTreeMap<String, String>) -> VersionSpec<'a> {
        VersionSpec {
            version,
            version_type: VersionType::Dev,
            component_refs: refs,
            distro: "rockylinux:9",
            el_version: 9,
            registry: "harbor.clyso.com",
            image_name: "ces/ceph/ceph",
            image_tag: None,
            user_name: "Jane",
            user_email: "jane@example.com",
        }
    }

    #[test]
    fn title_for_plain_and_suffixed_and_prefixed() {
        assert_eq!(
            version_title("20.2.1", VersionType::Dev).unwrap(),
            "Release Development version 20.2.1"
        );
        assert_eq!(
            version_title("ces-v20.2.1-rc.1", VersionType::Release).unwrap(),
            "Release General Availability CES version 20.2.1 (RC 1)"
        );
        assert_eq!(
            version_title("20.2.1-ga.1-hotfix", VersionType::Release).unwrap(),
            "Release General Availability version 20.2.1 (GA 1, HOTFIX)"
        );
    }

    #[test]
    fn title_for_uuidv7_is_created_at() {
        let v7 = crate::versions::validate::resolve_version(None);
        let title = version_title(&v7, VersionType::Dev).unwrap();
        assert!(
            title.starts_with("Release Development version created at "),
            "got: {title}"
        );
    }

    #[test]
    fn create_builds_descriptor_and_falls_back_image_tag() {
        let r = refs();
        let desc = create(&dev_spec("20.2.1", &r), &one_component(), &BTreeMap::new()).unwrap();
        assert_eq!(desc.image.tag, "20.2.1"); // falls back to the version
        assert_eq!(desc.components[0].repo, "https://github.com/ceph/ceph");
        assert_eq!(desc.components[0].git_ref, "v20.2.1");
        assert_eq!(desc.title, "Release Development version 20.2.1");
    }

    #[test]
    fn create_override_applies_only_to_known_components() {
        let r = refs();
        // Override of a known component changes the URL.
        let overrides = BTreeMap::from([(
            "ceph".to_string(),
            "https://internal.example/ceph".to_string(),
        )]);
        let desc = create(&dev_spec("20.2.1", &r), &one_component(), &overrides).unwrap();
        assert_eq!(desc.components[0].repo, "https://internal.example/ceph");

        // A ref to an unknown component errors even with an override present.
        let unknown_refs = BTreeMap::from([("dashboard".to_string(), "main".to_string())]);
        let unknown_overrides =
            BTreeMap::from([("dashboard".to_string(), "https://x/y".to_string())]);
        assert!(
            create(
                &dev_spec("20.2.1", &unknown_refs),
                &one_component(),
                &unknown_overrides,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn write_descriptor_lays_out_and_guards_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        let r = refs();
        let desc = create(&dev_spec("20.2.1", &r), &one_component(), &BTreeMap::new()).unwrap();

        let path = write_descriptor(&desc, root, VersionType::Dev)
            .await
            .unwrap();
        assert_eq!(path, root.join("dev/20.2.1.json"));
        // Round-trips back to an equal descriptor.
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(VersionDescriptor::parse(&written, None).unwrap(), desc);

        // A second write to the same path is refused.
        assert!(matches!(
            write_descriptor(&desc, root, VersionType::Dev).await,
            Err(CreateError::AlreadyExists { .. })
        ));
    }

    #[tokio::test]
    async fn helper_loads_components_and_applies_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let ceph_dir = base.join("ceph");
        tokio::fs::create_dir(&ceph_dir).await.unwrap();
        tokio::fs::write(
            ceph_dir.join("cbs.component.yaml"),
            "name: ceph\nrepo: https://default/ceph\nbuild:\n  get-version: gv.sh\n  deps: deps.sh\ncontainers:\n  path: c\n",
        )
        .await
        .unwrap();

        let r = refs();
        let desc = version_create_helper(
            &dev_spec("20.2.1", &r),
            &[base.to_owned()],
            &["ceph=https://override/ceph".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(desc.components[0].repo, "https://override/ceph");

        // A malformed override (no '=') errors.
        assert!(
            version_create_helper(
                &dev_spec("20.2.1", &r),
                &[base.to_owned()],
                &["no-equals-sign".to_string()],
            )
            .await
            .is_err()
        );
    }
}
