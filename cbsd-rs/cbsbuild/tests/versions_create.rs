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

//! End-to-end tests for `cbsbuild versions create` (review finding F1). Each
//! runs the built binary in a throwaway git repo via a subprocess, so the
//! handler's orchestration, error routing, and the trailing image-descriptor
//! note-gate are exercised without mutating the test process's cwd.

use std::path::Path;
use std::process::{Command, Output};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git is available");
    assert!(status.success(), "git {args:?} failed");
}

/// A throwaway git repo with a configured user and a single `ceph` component.
fn setup_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.name", "Test User"]);
    git(dir, &["config", "user.email", "test@example.com"]);

    let comp = dir.join("components").join("ceph");
    std::fs::create_dir_all(&comp).unwrap();
    // A complete cbs.component.yaml: the loader now strictly requires the
    // build/containers sections (review finding F2).
    std::fs::write(
        comp.join("cbs.component.yaml"),
        "name: ceph\nrepo: https://github.com/ceph/ceph\n\
         build:\n  rpm:\n    build: build_rpms.sh\n    release-rpm: get_release_rpm.sh\n\
         \x20 get-version: get_version.sh\n  deps: install_deps.sh\n\
         containers:\n  path: containers\n",
    )
    .unwrap();
    tmp
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cbsbuild"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("cbsbuild runs")
}

#[test]
fn writes_descriptor_and_notes_missing_image_desc() {
    let tmp = setup_repo();
    let out = run(
        tmp.path(),
        &[
            "versions",
            "create",
            "20.2.1",
            "-c",
            "ceph@v20.2.1",
            "--components-path",
            "components",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("version: 20.2.1"), "stdout: {stdout}");
    assert!(stdout.contains("Release Development version 20.2.1"));
    // No desc/ directory in the repo, so the (fixed) note fires.
    assert!(stdout.contains("image descriptor for version '20.2.1' missing"));

    let written = std::fs::read_to_string(tmp.path().join("_versions/dev/20.2.1.json")).unwrap();
    assert!(written.contains("\"ref\": \"v20.2.1\""));
    assert!(written.contains("https://github.com/ceph/ceph"));
    assert!(written.contains("\"schema_version\": 1"));
}

#[test]
fn uuidv7_path_skips_the_image_desc_note() {
    let tmp = setup_repo();
    let out = run(
        tmp.path(),
        &[
            "versions",
            "create",
            "-c",
            "ceph@v20.2.1",
            "--components-path",
            "components",
        ],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("version: "));
    assert!(stdout.contains("version created at "), "stdout: {stdout}");
    // A UUIDv7 has no M.m.p to key an image descriptor on — the note is skipped.
    assert!(!stdout.contains("image descriptor"), "stdout: {stdout}");
}

#[test]
fn an_existing_version_is_refused() {
    let tmp = setup_repo();
    let args = [
        "versions",
        "create",
        "20.2.1",
        "-c",
        "ceph@v20.2.1",
        "--components-path",
        "components",
    ];
    assert!(run(tmp.path(), &args).status.success());

    let second = run(tmp.path(), &args);
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("already exists"),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn the_required_component_flag_is_a_usage_error() {
    let tmp = setup_repo();
    let out = run(tmp.path(), &["versions", "create", "20.2.1"]);
    assert!(!out.status.success());
    // clap reports a usage error with exit code 2.
    assert_eq!(out.status.code(), Some(2));
}
