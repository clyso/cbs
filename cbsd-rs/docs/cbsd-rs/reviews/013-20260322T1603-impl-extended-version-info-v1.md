# 013 — Implementation Review: Extended Version Info (v1)

**Design:**
`docs/cbsd-rs/design/013-20260322T1210-extended-version-info.md`
(v2)

**Plan:**
`docs/cbsd-rs/plans/013-20260322T1504-extended-version-info.md`

**Commits reviewed:** `329269d..81d4e23` (plan commits 2, 3, 4)

**Verdict:** Needs fixes before merge

---

## Plan Correlation

All three commits map 1:1 to the plan. File lists match
exactly. Dependency ordering is correct (build.rs before
Hello version before container script). Each commit compiles
independently.

| Plan | SHA | Subject | Files | LOC |
|------|-----|---------|-------|-----|
| C2 | `329269d` | embed git version in all binaries | 7 | +104/-4 |
| C3 | `d098b5e` | report worker version in WS Hello | 6 | +52/-16 |
| C4 | `81d4e23` | prod build script, drop compose prod | 4 | +129/-67 |

Plan estimated ~200, ~100, ~150 authored lines. Actuals are
smaller but within reason — the build.rs is tighter than the
design sketch suggested.

---

## Findings

### F1 — Critical: debug artifact committed

`cbc/src/main.rs:1` contains `//foo`. This is a leftover
debug comment and must be removed.

### F2 — Important: design deviation on version JSON field

`cbsd-server/src/routes/workers.rs:42` annotates the
`version` field with `skip_serializing_if = "Option::is_none"`.
This means offline and disconnected workers have the field
**absent** from JSON rather than present as `null`.

The design explicitly states:

> Offline workers show `version: null`.

Clients that distinguish between `"version": null` and a
missing `version` key will see different behavior from what
the design promised. The `cbc` client uses
`#[serde(default)]` so it handles both, but third-party
consumers may not.

**Fix:** Remove the `skip_serializing_if` annotation from
`WorkerInfoResponse.version`.

### F3 — Important: missing `.gitignore` for `.git-version`

The `.git-version` file is designed to exist only inside
production builder containers. However, `cbsd-rs/.gitignore`
contains only `/target/`. If a developer runs the
Containerfile steps locally or creates the file for testing,
it could be accidentally staged and committed.

**Fix:** Add `.git-version` to `cbsd-rs/.gitignore`.

### F4 — Minor: Hello round-trip test lacks version assertion

`cbsd-proto/src/ws.rs:205-223`: The `worker_message_hello_
round_trip` test serializes a Hello with `version: Some(
"0.1.0+gtest123")` but only asserts `arch` after
deserialization. The `version` field is not verified to
survive the round trip.

Similarly, the `hello_arm64_alias` test (line 225-234)
implicitly proves that `#[serde(default)]` works for a
Hello without `version` (the JSON has no version key and
deserialization succeeds), but it doesn't assert that
`version == None` — so this backwards-compat guarantee
is untested.

**Suggested:** Add `version` assertions to both tests.

### F5 — Minor: silent acceptance of versionless workers

`cbsd-server/src/ws/handler.rs:240-249`: When a worker
sends `Hello` without a `version` field (`worker_version`
is `None`), the version skew check is skipped entirely.
The INFO log at line 235 shows `worker_version = "unknown"`
(via `unwrap_or`), but there is no distinct log event
indicating the worker is running legacy firmware with no
version reporting.

A `DEBUG`-level log for `worker_version.is_none()` would
help operators identify workers that need upgrading.

### F6 — Observation: version lost on disconnect

`WorkerState::Disconnected` does not carry a `version`
field, so the worker's version becomes `None` the moment
it disconnects. This is consistent with the design (which
only adds `version` to `Connected`), but means the workers
API cannot report the last-known version of a recently
disconnected worker.

Not a bug — just a limitation to be aware of if the
version display turns out to be operationally important
for disconnected workers.

---

## What's Done Well

**Build.rs approach.** Using `std::env::var("CARGO_MANIFEST_
DIR")` instead of the design's `env!("CARGO_MANIFEST_DIR")`
is correct and idiomatic for build scripts. The
`rerun-if-changed` directive ensures correct cache
invalidation.

**Version skew detection.** The INFO + WARN logging pattern
in the server handler is clean. Logging the worker version
in the connection event (line 228-237) makes it greppable
without a separate log event.

**Build script.** `container/build-cbsd-rs.sh` is well-
structured: `set -euo pipefail`, clear usage, fallback
`|| echo unknown` for non-git contexts, `CBS_IMAGE_PREFIX`
override for custom registries. The `--push` flag is a
nice operational touch.

**Compose cleanup.** Removing the production profiles is
clean — 56 lines deleted, comment and usage updated.

**Serde annotations.** `#[serde(default, skip_serializing_
if = "Option::is_none")]` on the proto Hello `version`
field correctly handles both directions: new workers send
it, old workers don't, and the server doesn't send it back
in Welcome.

---

## Summary

| Severity | Count | Action |
|----------|-------|--------|
| Critical | 1 | Must fix (F1) |
| Important | 2 | Should fix (F2, F3) |
| Minor | 2 | Should consider (F4, F5) |
| Observation | 1 | Awareness only (F6) |

F1 is a clear ship-blocker. F2 is a design deviation that
could cause subtle API incompatibilities. F3 is a safety
net worth adding.
