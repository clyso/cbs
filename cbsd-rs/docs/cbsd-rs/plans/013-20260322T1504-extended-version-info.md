# 013 — Extended Version Info: Implementation Plan

**Design:**
`docs/cbsd-rs/design/013-20260322T1210-extended-version-info.md`
(v2, approved)

## Commit Breakdown

5 commits, ordered by dependency.

---

### Commit 1: `cbsd-rs/docs: add extended version info design and plan`

**Documentation only**

Design document (v2), implementation plan, and all
design/plan reviews.

**Files:**

| File | Change |
|------|--------|
| `docs/cbsd-rs/design/013-*` | Design |
| `docs/cbsd-rs/plans/013-*` | This plan |
| `docs/cbsd-rs/reviews/013-*` | All reviews |

---

### Commit 2: `cbsd-rs: embed git version in all binaries via build.rs`

**~200 authored lines**

Add `build.rs` to all three binary crates that reads a
`.git-version` file from the workspace root. If absent or
empty, defaults to `unknown`. Wire the `CBS_BUILD_META`
env var into a `VERSION` constant. Update clap `version`
attributes on all three CLIs. Update the server's health
endpoint to include the version string.

**Files:**

| File | Change |
|------|--------|
| `cbsd-server/build.rs` | New: read `.git-version`, set `CBS_BUILD_META` |
| `cbsd-worker/build.rs` | New: same |
| `cbc/build.rs` | New: same |
| `cbsd-server/src/main.rs` | `VERSION` const at crate scope; `#[command(version = VERSION)]` |
| `cbsd-worker/src/main.rs` | `VERSION` const; `#[command(version = VERSION)]` |
| `cbc/src/main.rs` | `VERSION` const; `#[command(version = VERSION)]` |
| `cbsd-server/src/app.rs` | Health endpoint uses `crate::VERSION` to return `{"status":"ok","version":"..."}` |

**Note (from review C1):** `VERSION` is defined at crate
scope in `main.rs`. The `health()` function in `app.rs`
accesses it as `crate::VERSION`.

**Verification:**

```bash
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
# Verify: cargo run --bin cbc -- --version
#   → cbc 0.1.0+unknown
```

---

### Commit 3: `cbsd-rs: report worker version in WebSocket Hello`

**~100 authored lines**

Add `version: Option<String>` to the `Hello` WebSocket
message in `cbsd-proto`. The worker includes its
`VERSION` in the Hello. The server logs the worker
version on connect (INFO), warns on version mismatch
(WARN). Store version in `WorkerState::Connected`.
Expose in `GET /api/workers` response. Update `cbc`
worker list display.

**Files:**

| File | Change |
|------|--------|
| `cbsd-proto/src/ws.rs` | `Hello` gains `version: Option<String>` with `serde(default)` |
| `cbsd-worker/src/ws/handler.rs` | Include `VERSION` in Hello message |
| `cbsd-server/src/ws/handler.rs` | Log worker version; warn on skew vs server version |
| `cbsd-server/src/ws/liveness.rs` | `WorkerState::Connected` gains `version: Option<String>` |
| `cbsd-server/src/routes/workers.rs` | `WorkerInfoResponse` gains `version` field |
| `cbc/src/worker.rs` | Display version in worker list table |

**Verification:**

```bash
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

---

### Commit 4: `container: add production build script and drop compose prod profiles`

**~150 authored lines**

New `container/build-cbsd-rs.sh` script that runs
`git describe --always --match=''` on the host, passes
`--build-arg GIT_VERSION=<sha>` to `podman build`.
Update `ContainerFile.cbsd-rs` rust-builder stage to
accept `ARG GIT_VERSION` and write `.git-version` file
before `cargo build`. Remove prod profiles from
`podman-compose.cbsd-rs.yaml`. Update README to document
the production build workflow.

**Files:**

| File | Change |
|------|--------|
| `container/build-cbsd-rs.sh` | New: production build script |
| `container/ContainerFile.cbsd-rs` | `ARG GIT_VERSION`, `RUN echo` in rust-builder stage |
| `podman-compose.cbsd-rs.yaml` | Remove `server` and `worker` prod services |
| `cbsd-rs/README.md` | Document prod vs dev deployment, build script usage |

**Verification:**

- `container/build-cbsd-rs.sh --help` works
- `podman-compose -f podman-compose.cbsd-rs.yaml config
  --profile dev` parses without error
- Production profiles no longer listed

---

### Commit 5: `cbsd-rs/docs: add implementation reviews`

**Documentation only**

Post-implementation review documents.

**Files:**

| File | Change |
|------|--------|
| `docs/cbsd-rs/reviews/013-*-impl-*` | Implementation review(s) |

---

## Dependency Graph

```
Commit 1 (docs)
    ↓
Commit 2 (build.rs + VERSION in all binaries + health)
    ↓
Commit 3 (Hello version + server logging + workers API)
    ↓
Commit 4 (container build script + compose cleanup)
    ↓
Commit 5 (impl reviews)
```

Commits 2 and 3 are strictly ordered — the worker needs
`VERSION` from commit 2 to include it in Hello. Commit 4
is independent of commit 3 (it changes container/compose
files, not Rust code) but logically comes after since the
build script produces the `.git-version` file that commit
2's `build.rs` reads.

## Progress

| # | Commit | Status |
|---|--------|--------|
| 1 | docs: design, plan, reviews | Pending |
| 2 | build.rs + VERSION + health | Pending |
| 3 | Hello version + workers API | Pending |
| 4 | build script + compose cleanup | Pending |
| 5 | docs: implementation reviews | Pending |
