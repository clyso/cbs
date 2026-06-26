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

//! End-to-end tests for the host runner (design 009), proving it spins a real
//! builder container and recovers the report across the scratch mount. Ignored
//! by default: they need podman on PATH and network to pull `alpine`. Run with
//! `cargo test -p cbscore --test runner_podman -- --ignored`.
//!
//! The mounted "cbsbuild" is a small shell stub (a glibc dev build of the real
//! binary could not run in musl alpine); it exercises the runner's host-side
//! mechanics — mount/argv assembly, the report round-trip, cleanup, and
//! cancellation — without a real toolchain.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use cbscore::runner::{CancellationToken, RunOpts, RunnerError, run};
use cbscore::types::{
    Config, PathsConfig, VersionComponent, VersionDescriptor, VersionImage, VersionSignedOffBy,
};

/// A throwaway build environment: config, secrets, scratch, components, a
/// descriptor file, and a stub `cbsbuild` with the given script body.
struct Fixture {
    _dir: tempfile::TempDir,
    config: Config,
    desc_path: Utf8PathBuf,
    scratch: Utf8PathBuf,
    stub: Utf8PathBuf,
}

async fn setup(stub_body: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = Utf8Path::from_path(dir.path()).unwrap().to_owned();

    let scratch = base.join("scratch");
    let containers = base.join("containers");
    let components = base.join("components");
    for d in [&scratch, &containers, &components] {
        tokio::fs::create_dir_all(d).await.unwrap();
    }

    // An empty-but-valid secrets file (get_secrets requires at least one).
    let secrets = base.join("secrets.yaml");
    tokio::fs::write(&secrets, "{}\n").await.unwrap();

    // The stub mounted as the container's cbsbuild, made executable.
    let stub = base.join("cbsbuild-stub.sh");
    tokio::fs::write(&stub, stub_body).await.unwrap();
    let mut perm = tokio::fs::metadata(&stub).await.unwrap().permissions();
    perm.set_mode(0o755);
    tokio::fs::set_permissions(&stub, perm).await.unwrap();

    // A descriptor whose distro is the (small, musl) image we actually run.
    let desc = VersionDescriptor {
        schema_version: 1,
        version: "20.2.1".to_string(),
        title: "Release version 20.2.1".to_string(),
        signed_off_by: VersionSignedOffBy {
            user: "Jane".to_string(),
            email: "jane@example.com".to_string(),
        },
        image: VersionImage {
            registry: "harbor.clyso.com".to_string(),
            name: "ces/ceph/ceph".to_string(),
            tag: "20.2.1".to_string(),
        },
        components: vec![VersionComponent {
            name: "ceph".to_string(),
            repo: "https://github.com/ceph/ceph".to_string(),
            git_ref: "v20.2.1".to_string(),
        }],
        distro: "alpine".to_string(),
        el_version: 9,
    };
    let desc_path = base.join("20.2.1.json");
    tokio::fs::write(&desc_path, desc.to_json_pretty().unwrap())
        .await
        .unwrap();

    let config = Config {
        schema_version: 1,
        paths: PathsConfig {
            components: vec![components],
            scratch: scratch.clone(),
            scratch_containers: containers,
            ccache: None,
        },
        storage: None,
        signing: None,
        logging: None,
        secrets: vec![secrets],
        vault: None,
    };

    Fixture {
        _dir: dir,
        config,
        desc_path,
        scratch,
        stub,
    }
}

fn opts(fixture: &Fixture, name: &str, cancel: CancellationToken) -> RunOpts {
    RunOpts {
        run_name: Some(name.to_string()),
        replace_if_exists: true,
        timeout: Duration::from_secs(120),
        skip_build: false,
        force: false,
        tls_verify: true,
        debug: false,
        log_file: None,
        cancel,
        cbsbuild_bin: fixture.stub.clone(),
    }
}

async fn podman_rm(name: &str) {
    let _ = tokio::process::Command::new("podman")
        .args(["rm", "-f", name])
        .output()
        .await;
}

#[tokio::test]
#[ignore = "requires podman and network to pull alpine"]
async fn spins_the_container_and_round_trips_the_report() {
    // The stub writes a skipped report to the scratch mount and exits 0.
    let stub = "#!/bin/sh\ncat > /runner/scratch/build-report.json <<'JSON'\n{\"report_version\":1,\"version\":\"20.2.1\",\"skipped\":true,\"container_image\":null,\"release_descriptor\":null,\"components\":[]}\nJSON\n";
    let fx = setup(stub).await;
    let name = format!("cbscore-runner-ok-{}", std::process::id());

    let result = run(
        &fx.desc_path,
        &fx.config,
        opts(&fx, &name, CancellationToken::new()),
    )
    .await;
    podman_rm(&name).await;

    let report = result.expect("runner should succeed").expect("a report");
    assert!(report.skipped);
    assert_eq!(report.version, "20.2.1");
    // The report was unlinked from the scratch mount after the read (design 009).
    assert!(
        !tokio::fs::try_exists(fx.scratch.join("build-report.json"))
            .await
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "requires podman and network to pull alpine"]
async fn a_nonzero_exit_carries_the_partial_report() {
    // The stub writes a report, then exits non-zero — the report must ride the
    // error, not be discarded (the Python bug 009 fixes).
    let stub = "#!/bin/sh\ncat > /runner/scratch/build-report.json <<'JSON'\n{\"report_version\":1,\"version\":\"20.2.1\",\"skipped\":false,\"container_image\":null,\"release_descriptor\":null,\"components\":[]}\nJSON\nexit 1\n";
    let fx = setup(stub).await;
    let name = format!("cbscore-runner-fail-{}", std::process::id());

    let result = run(
        &fx.desc_path,
        &fx.config,
        opts(&fx, &name, CancellationToken::new()),
    )
    .await;
    podman_rm(&name).await;

    match result {
        Err(RunnerError::NonZeroExit { report, .. }) => {
            let report = report.expect("the partial report rides the error");
            assert_eq!(report.version, "20.2.1");
        }
        other => panic!("expected NonZeroExit, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires podman and network to pull alpine"]
async fn firing_the_cancel_token_stops_the_container() {
    // The stub sleeps; firing the token mid-run must stop it and return Cancelled.
    let fx = setup("#!/bin/sh\nsleep 60\n").await;
    let name = format!("cbscore-runner-cancel-{}", std::process::id());
    let token = CancellationToken::new();

    let firing = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        firing.cancel();
    });

    let result = run(&fx.desc_path, &fx.config, opts(&fx, &name, token)).await;
    podman_rm(&name).await;

    assert!(
        matches!(result, Err(RunnerError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
}
