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

//! The in-container builder pipeline (design 007). This runs **inside** the
//! builder container (the host runner that spawns it is design 009); it writes
//! `build-report.json` to the scratch mount, which the host reads back.
//!
//! This is the C2a keystone skeleton: [`Builder::run`] writes a `skipped`
//! report and returns, which is enough to prove the host/container round-trip.
//! The real stages — prepare → rpmbuild → sign → upload → container image —
//! land in C2b–C6, each replacing part of this stub.

use camino::Utf8PathBuf;
use cbscore_types::{BuildArtifactReport, Config, VersionDescriptor};
use tracing::debug;

use crate::types::tracing_targets;

/// The filename the report is written under, on the scratch mount.
pub const BUILD_REPORT_FILE: &str = "build-report.json";

/// Build behaviour flags threaded from the CLI (design 010). The full pipeline
/// (C2b–C6) consumes these; the keystone only records them.
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    pub skip_build: bool,
    pub force: bool,
    pub tls_verify: bool,
}

/// An error from the in-container build.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    #[error("error writing build report to '{path}'")]
    WriteReport {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("error serialising build report")]
    Serialize(#[source] serde_json::Error),
}

/// The in-container build orchestrator (design 007).
pub struct Builder<'a> {
    config: &'a Config,
    desc: &'a VersionDescriptor,
    opts: BuildOptions,
}

impl<'a> Builder<'a> {
    pub fn new(config: &'a Config, desc: &'a VersionDescriptor, opts: BuildOptions) -> Self {
        Self { config, desc, opts }
    }

    /// Run the in-container build and write its report to the scratch mount.
    ///
    /// Keystone behaviour: emit a `skipped` report. The decision flow (skopeo
    /// short-circuit, release reuse, the build stages) lands in C2b–C6.
    pub async fn run(&self) -> Result<BuildArtifactReport, BuilderError> {
        debug!(
            target: tracing_targets::BUILDER,
            "building {} (skip_build={}, force={}, tls_verify={})",
            self.desc.version, self.opts.skip_build, self.opts.force, self.opts.tls_verify
        );
        let report = BuildArtifactReport {
            report_version: 1,
            version: self.desc.version.clone(),
            skipped: true,
            container_image: None,
            release_descriptor: None,
            components: vec![],
        };
        self.write_report(&report).await?;
        Ok(report)
    }

    async fn write_report(&self, report: &BuildArtifactReport) -> Result<(), BuilderError> {
        let path = self.config.paths.scratch.join(BUILD_REPORT_FILE);
        let json = serde_json::to_string_pretty(report).map_err(BuilderError::Serialize)?;
        tokio::fs::write(&path, json)
            .await
            .map_err(|source| BuilderError::WriteReport { path, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;

    fn config_with_scratch(scratch: &Utf8Path) -> Config {
        Config {
            schema_version: 1,
            paths: cbscore_types::PathsConfig {
                components: vec!["components".into()],
                scratch: scratch.to_owned(),
                scratch_containers: "/var/lib/containers".into(),
                ccache: None,
            },
            storage: None,
            signing: None,
            logging: None,
            secrets: vec![],
            vault: None,
        }
    }

    fn descriptor() -> VersionDescriptor {
        VersionDescriptor {
            schema_version: 1,
            version: "20.2.1".to_string(),
            title: "Release version 20.2.1".to_string(),
            signed_off_by: cbscore_types::VersionSignedOffBy {
                user: "Jane".to_string(),
                email: "jane@example.com".to_string(),
            },
            image: cbscore_types::VersionImage {
                registry: "harbor.clyso.com".to_string(),
                name: "ces/ceph/ceph".to_string(),
                tag: "20.2.1".to_string(),
            },
            components: vec![],
            distro: "rockylinux:9".to_string(),
            el_version: 9,
        }
    }

    #[tokio::test]
    async fn run_writes_a_skipped_report_to_the_scratch_mount() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = Utf8Path::from_path(dir.path()).unwrap();
        let config = config_with_scratch(scratch);
        let desc = descriptor();
        let opts = BuildOptions {
            skip_build: false,
            force: false,
            tls_verify: true,
        };

        let report = Builder::new(&config, &desc, opts).run().await.unwrap();
        assert!(report.skipped);
        assert_eq!(report.version, "20.2.1");

        // The report landed on the scratch mount where the host runner reads it.
        let written = tokio::fs::read_to_string(scratch.join(BUILD_REPORT_FILE))
            .await
            .unwrap();
        let parsed: BuildArtifactReport = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed, report);
    }
}
