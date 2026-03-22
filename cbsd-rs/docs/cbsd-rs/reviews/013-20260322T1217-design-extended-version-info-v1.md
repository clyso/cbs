# Design Review: 013 — Extended Version Info (v1)

**Document:**
`design/013-20260322T1210-extended-version-info.md`

---

## Summary

The design addresses a real operational gap — builds are
indistinguishable without commit info. The overall
approach (compile-time git SHA embedding, health endpoint
version, Hello message version) is sound and follows
established patterns. Two blockers: the `rerun-if-changed`
path is wrong for this repo layout, and the dirty-check
command misses staged changes. Several minor issues with
the build.rs robustness.

**Verdict: Approve with conditions.**

---

## Blockers

### B1 — `rerun-if-changed` path is wrong

The design's `build.rs` uses:

```rust
println!("cargo:rerun-if-changed=../.git/HEAD");
println!("cargo:rerun-if-changed=../.git/refs/");
```

`build.rs` runs from the crate directory (e.g.,
`cbsd-rs/cbsd-server/`). `../.git/` resolves to
`cbsd-rs/.git/` — which does not exist. The `.git/`
directory is at the **repo root** (`cbs.git/.git/`),
which is **two** levels up from the crate directory.

The correct paths are:

```rust
println!("cargo:rerun-if-changed=../../.git/HEAD");
println!("cargo:rerun-if-changed=../../.git/refs/");
```

Or better, detect the git dir dynamically:

```rust
let git_dir = std::process::Command::new("git")
    .args(["rev-parse", "--git-dir"])
    .output()
    .ok()
    .and_then(|o| String::from_utf8(o.stdout).ok())
    .map(|s| s.trim().to_string());

if let Some(ref gd) = git_dir {
    println!("cargo:rerun-if-changed={gd}/HEAD");
    println!("cargo:rerun-if-changed={gd}/refs/");
}
```

This handles worktrees (where `.git` is a file pointing
to the worktree's git dir, not a directory), non-standard
repo layouts, and the container build case (where git is
absent — the command fails, no `rerun-if-changed` is
emitted, and the fallback value is used).

### B2 — `git diff --quiet` misses staged changes

The design uses `git diff --quiet` for dirty detection.
This only checks the working tree against the index —
it does NOT detect staged-but-uncommitted changes.

A developer who runs `git add file.rs` and then
`cargo build` will get a clean version string even
though the build includes uncommitted staged changes.

**Fix:** Use `git diff-index --quiet HEAD` instead,
which checks both staged and unstaged changes against
HEAD. Or use `git status --porcelain` and check for
non-empty output (more comprehensive but slightly
slower).

---

## Major Concerns

### M1 — `WorkerState::Connected` must gain a `version` field

The design says the server "stores the reported version
in its in-memory worker state" and the workers endpoint
includes `version`. Looking at the actual code:

```rust
pub enum WorkerState {
    Connected {
        registered_worker_id: String,
        worker_name: String,
        arch: Arch,
        cores_total: u32,
        ram_total_mb: u64,
    },
    // ...
}
```

Adding `version: Option<String>` to `Connected` (and
possibly `Disconnected`) touches the `WorkerState` enum
and every pattern match on it. The design's Files Changed
table lists `cbsd-server/src/ws/handler.rs` for "Log
worker version on connect" but does not list
`cbsd-server/src/ws/liveness.rs` where `WorkerState` is
defined, nor `cbsd-server/src/routes/workers.rs` where
`WorkerInfoResponse` is defined.

**Fix:** Add `liveness.rs` and `workers.rs` to the Files
Changed table. Note that `WorkerInfoResponse` needs a
`version: Option<String>` field.

### M2 — `cbc worker list` must be updated for the version field

The `cbc` client's `WorkerInfo` deserialization struct
and the list table format will need updating to show
worker version. The design's Files Changed table does
not mention any `cbc` changes beyond `cbc/build.rs` and
`cbc/src/main.rs` (for `--version`). The worker list
display is an additional change.

**Fix:** Either add `cbc/src/worker.rs` to the Files
Changed table or explicitly defer the cbc list update.

---

## Minor Issues

- **`git rev-parse --short HEAD` in container builds.**
  The design correctly handles this via `CBS_GIT_SHA`
  env var fallback. However, the `build.rs` should check
  the env var **first** (before attempting `git`), not
  as a fallback after git failure. In containers, the
  git command may take several hundred milliseconds to
  fail (scanning PATH, etc.). Checking the env var first
  is instantaneous.

- **`git rev-parse` runs in the workspace root, not
  the crate directory.** The `build.rs` runs `git` with
  no explicit `-C` flag. `git` searches upward for the
  `.git` directory, so this works as long as any ancestor
  has `.git/`. In the container case (no `.git/`
  anywhere), it fails to the fallback. This is correct
  but worth a comment.

- **The design says "abbreviated SHA is 7 characters".**
  `git rev-parse --short HEAD` defaults to the minimum
  unambiguous length (typically 7-12 chars depending on
  repo size). For consistency, use `--short=7` to force
  exactly 7 characters.

- **Duplicated `build.rs` across 3 crates.** The design
  acknowledges this and says "duplication is acceptable."
  An alternative: a workspace-level `build.rs` doesn't
  exist in Cargo, but a shared crate used as a
  `[build-dependencies]` could centralize this.
  At ~20 lines, duplication is the right call.

- **`WorkerInfoResponse` already has no `version` field.**
  Adding it as `Option<String>` is straightforward but
  the workers listing code in `routes/workers.rs` builds
  the response from the DB `WorkerRow` merged with the
  in-memory `WorkerState`. The version is only in
  `WorkerState` (in-memory, not persisted). Offline
  workers will show `version: null`. This is acceptable
  but should be documented.

- **`Hello` message serde compatibility.** Adding
  `version: Option<String>` with `#[serde(default)]` to
  `Hello` is safe — old workers that don't send it
  deserialize with `None`. The server's hello handler
  must destructure the new field. This mirrors the
  existing `build_report` pattern on `BuildFinished`.

---

## Suggestions

- **Consider `vergen` crate.** The `vergen` crate is
  the standard Rust solution for compile-time git info.
  It handles all the edge cases (worktrees, shallow
  clones, CI environments, missing git) and provides
  cargo env vars like `VERGEN_GIT_SHA`,
  `VERGEN_GIT_DIRTY`, `VERGEN_GIT_DESCRIBE`. It's a
  build dependency only (zero runtime cost). The
  hand-rolled `build.rs` reinvents this wheel.

  Counter-argument: `vergen` adds a dependency tree
  (it pulls in `git2` or `gix`). The hand-rolled
  version calls the `git` CLI, which is simpler but
  requires `git` on PATH at build time. Either is
  acceptable — the design should explicitly state the
  trade-off.

- **Consider including build timestamp.** The version
  string `0.1.0+g3a7f2b1` doesn't tell you *when* it
  was built — two builds from the same commit at
  different times are indistinguishable. Adding a build
  timestamp (ISO 8601, UTC) to the health endpoint
  response (not the version string) would help:
  `{"status":"ok","version":"0.1.0+g3a7f2b1",
  "built_at":"2026-03-22T12:00:00Z"}`.

---

## Strengths

- **The semver build-metadata format** (`+g<sha>`) is
  correct per SemVer 2.0.0 — build metadata does not
  affect version precedence.

- **Container build handling is sound.** The `CBS_GIT_SHA`
  env var fallback with `ARG`/`ENV` in the Containerfile
  is the standard pattern for passing build-time info
  into multi-stage container builds.

- **`Hello` message version exchange** is the right place
  for worker-server version visibility. Logging version
  skew without enforcement is the correct first step.

- **No protocol version bump needed.** The `serde(default)`
  pattern for backwards compatibility is established and
  correct.

---

## Open Questions

1. **Should the server log a warning when worker version
   differs from server version?** The design says "just
   logging" but doesn't specify the log level. A
   `tracing::info!` on every connect is fine; a
   `tracing::warn!` on version mismatch would be more
   operationally useful.

2. **Should the health endpoint be extended to include
   the server's uptime?** This is a common addition that
   helps operators distinguish "just restarted" from
   "running for days." Not blocking but worth considering
   alongside the version addition.
