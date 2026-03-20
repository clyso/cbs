# Phase 0 — Repository Scaffolding

## Progress

| Item | Status |
|------|--------|
| Commit 0: Create cbsd-rs/ directory with CLAUDE.md | Done |

## Goal

Create the `cbsd-rs/` workspace directory at the repository root and populate
it with a `CLAUDE.md` that provides implementation context for all future
Claude sessions working on the Rust codebase.

## Depends on

Nothing. This is the first phase.

## Commit 0: Create cbsd-rs/ directory with CLAUDE.md

**Create `cbsd-rs/` directory** at the repository root (alongside existing
`cbsd/`, `cbscore/`, `cbc/`, `crt/`, etc.).

**Create `cbsd-rs/CLAUDE.md`** containing:

1. **Project overview** — what cbsd-rs is (Rust reimplementation of the CBS
   daemon), its relationship to the existing Python `cbsd/`, and the three
   crates in the workspace (`cbsd-proto`, `cbsd-server`, `cbsd-worker`).

2. **Skill references** — instructions to always consult:
   - `/rust-2024` for all Rust code (2024 edition, axum, tokio, serde,
     sqlx, tracing, error handling, async patterns)
   - `/git-commit-messages` for commit message formatting and logical change
     boundaries (Ceph project conventions)
   - `/git-autonomous-commits` for autonomous git operations (staging,
     pre-commit, self-review, commit strategy)

3. **Build & test commands:**
   - `cargo build --workspace`
   - `cargo test --workspace`
   - `cargo clippy --workspace`
   - `cargo fmt --check`
   - `cargo sqlx prepare` (after migration or query changes)

4. **Git conventions:**
   - DCO sign-off: `git -c commit.gpgsign=false commit -s`
   - Never GPG-sign commits autonomously
   - No Co-Authored-By lines
   - Separate `git add` and `git commit` commands

5. **Key architecture pointers:**
   - Design documents at `_docs/cbsd-rs/design/` (authoritative)
   - Implementation plans at `_docs/cbsd-rs/plans/` (update progress after
     each commit)
   - SQLite with sqlx (compile-time checked queries, offline cache in
     `.sqlx/`, `SQLX_OFFLINE=true` for CI)
   - Workspace layout: `cbsd-proto` (shared types, no IO), `cbsd-server`
     (axum REST + WS), `cbsd-worker` (WS client + subprocess)

6. **Correctness invariants** (easy to miss, document prominently):
   - Dispatch mutex held across SQLite write, released before WS send
   - `max_connections = 4` pool sizing to prevent deadlock
   - `foreign_keys=ON` per-connection pragma (sqlx pool option)
   - `trace_id` generated at dispatch time under mutex, persisted in
     `builds.trace_id`, propagated to worker for cross-boundary correlation
   - `tower-sessions-sqlx-store` `.migrate().await` called after sqlx
     migrations but before router is built (creates `tower_sessions` table)
   - Watch senders stored in `AppState.log_watchers` (not `ActiveBuild`),
     created at dispatch, dropped by `build_finished` and startup recovery

7. **sqlx offline query cache:** `.sqlx/` directory lives at workspace root
   (`cbsd-rs/.sqlx/`). **After any commit that adds sqlx queries, re-run
   `cargo sqlx prepare --workspace` and include `.sqlx/` in the commit.**
   CI builds use `SQLX_OFFLINE=true`.

8. **Implementation guidance:**
   - The design docs are authoritative — if code and design disagree, fix
     the code
   - Check the plan progress tables before starting work
   - Update plan progress tables after completing each commit
   - Each commit must compile and pass tests
   - Consult `cbsdcore/src/cbsdcore/versions.py` for Python BuildDescriptor
     when porting types
   - Consult `cbsd/cbslib/core/permissions.py` for Python permission model
     as reference

**Testable:** `cbsd-rs/` directory exists with `CLAUDE.md`. No Cargo files
yet (those come in Phase 1).
