# Design Review: 013 — Extended Version Info (v2)

**Document:**
`design/013-20260322T1210-extended-version-info.md`
(v2 — addresses review v1 + user feedback)

---

## Summary

All v1 findings are resolved. The `.git-version` file
approach eliminates the `rerun-if-changed` path issue
and the dirty-detection problem entirely. Dev builds
always show `unknown`; production builds get the SHA via
a container build arg. The design is clean and minimal.

No blockers. One minor concern.

**Verdict: Approved.**

---

## Prior Findings Disposition

| v1 Finding | Status |
|---|---|
| B1 — `rerun-if-changed` path wrong | Resolved (`.git-version` file, `CARGO_MANIFEST_DIR.parent()`) |
| B2 — `git diff --quiet` misses staged | Resolved (no dirty detection; dev always `unknown`) |
| M1 — `liveness.rs` + `workers.rs` missing from files | Resolved (listed) |
| M2 — `cbc/src/worker.rs` missing | Resolved (listed) |

---

## Blockers

None.

---

## Major Concerns

None.

---

## Minor Issues

- **`env!("CARGO_MANIFEST_DIR")` in `build.rs`.**
  The design uses `env!("CARGO_MANIFEST_DIR")` — this
  is a compile-time macro that works in `build.rs`
  because Cargo sets it for the build script's
  compilation. It resolves to the crate directory
  (e.g., `cbsd-rs/cbsd-server/`). `.parent()` gives
  the workspace root (`cbsd-rs/`). Correct. However,
  `std::env::var("CARGO_MANIFEST_DIR")` (runtime) is
  the more conventional approach in `build.rs` and
  avoids the subtle distinction. Either works.

- **`git describe --always --match=''` output length.**
  The command defaults to minimum unambiguous SHA
  length (7-12 chars depending on repo). For
  deterministic output, use `--abbrev=7`. Not critical
  — the version string works with any length.

- **Podman-compose removing prod profiles.** The design
  says prod profiles are removed. This is a user-facing
  change — operators currently using compose for
  production must switch to the new `build-cbsd-rs.sh`
  script. The README update should clearly document the
  migration path.

---

## Strengths

- **The `.git-version` file approach is cleaner than the
  v1 `build.rs` git invocation.** It avoids the entire
  class of problems with git availability, PATH issues,
  worktree paths, and container build contexts. The
  file exists only in the builder stage, never committed.

- **Dev builds always showing `unknown` is the right
  call.** Attempting to resolve git info in dev adds
  complexity (worktrees, dirty detection, PATH) for
  minimal operational value. The version info matters
  in production — the design correctly focuses there.

- **`CARGO_MANIFEST_DIR.parent()` for workspace root
  is reliable.** All three crates are direct children
  of the workspace root.

- **`rerun-if-changed` points to the `.git-version`
  file.** If the file appears or changes, Cargo re-runs
  `build.rs`. If it doesn't exist (dev), Cargo still
  re-runs on first build but caches thereafter.

- **Container build flow is clean.** Host `git describe`
  → `--build-arg` → `RUN echo` → `.git-version` → Cargo
  reads at build time → binary embeds it. The file
  exists only in the ephemeral builder stage.

- **Worker version in `Hello` + WARN on skew** is the
  right level of enforcement. Blocking on version
  mismatch would be too disruptive for rolling upgrades.

- **Files Changed table is now complete.** All affected
  files across server, worker, cbc, proto, liveness,
  routes, container, compose, and README are listed.

- **`container/build-cbsd-rs.sh`** as a dedicated
  production build script follows the existing pattern
  (`container/build.sh` for the Python cbsd).
