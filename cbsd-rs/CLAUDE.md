# CLAUDE.md — cbsd-rs

Rust reimplementation of the CBS build service daemon (`cbsd/`). Replaces the
Python 3 + FastAPI + Celery + Redis stack with Rust + axum + SQLite + WebSocket.

## Skills

Always consult these skills during implementation:

- **`/rust-2024`** — Rust 2024 edition: project structure, error handling,
  trait design, async patterns (tokio), axum idioms, serde, sqlx, tracing.
- **`/git-commit-messages`** — Commit message formatting and logical change
  boundaries. Ceph project conventions.
- **`/git-autonomous-commits`** — Autonomous git operations: staging,
  pre-commit checks, self-review, commit strategy.
- **`/cbsd-rs-docs`** — Where to place and how to name design documents,
  plans, and review documents for cbc and cbsd-rs packages.

## Workspace Layout

```
cbsd-rs/
├── Cargo.toml          # workspace root
├── Cargo.lock
├── .sqlx/              # sqlx offline query cache (committed)
├── migrations/         # sqlx SQL migrations (embedded by server)
├── scripts/            # cbscore-wrapper.py (Python subprocess bridge)
├── cbsd-proto/         # shared types crate (no IO, no async)
├── cbsd-server/        # server binary (axum REST + WebSocket)
└── cbsd-worker/        # worker binary (WS client + subprocess)
```

- **`cbsd-proto`** — BuildDescriptor, Arch, Priority, BuildState, WebSocket
  message types, scope types. Zero IO dependencies (serde, serde_json, chrono
  only). Both server and worker depend on this — compile-time wire format
  agreement.
- **`cbsd-server`** — REST API (axum), Google OAuth, PASETO tokens, RBAC
  permissions, build queue with priority lanes, WebSocket handler for workers,
  SSE log streaming, SQLite persistence (sqlx).
- **`cbsd-worker`** — WebSocket client (tokio-tungstenite), reconnection with
  exponential backoff, build executor (spawns cbscore Python wrapper as
  subprocess), component tarball unpacking, SIGTERM/SIGKILL process group
  management.

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo fmt --check
```

After any migration or query change:


```bash
DATABASE_URL=sqlite:///tmp/cbsd-dev.db cargo sqlx prepare --workspace
```

CI builds use `SQLX_OFFLINE=true` (reads from committed `.sqlx/` cache).

## Pre-Commit Checks

Before every commit, run these checks **in order** on all modified files:

```bash
cargo fmt --all                    # 1. format
cargo clippy --workspace           # 2. lint (fix any warnings)
cargo check --workspace            # 3. compile check (with SQLX_OFFLINE=true if needed)
```

All three must pass with zero errors and zero warnings before staging.

## Git Conventions

```bash
# All commits use this form:
git -c commit.gpgsign=false commit -s
```

- DCO sign-off (`-s`) required on every commit
- Never GPG-sign commits autonomously
- Autonomous commits where Claude made changes MUST include exactly one
  `Co-authored-by` trailer after the message body. Never stack multiples.
  Use the model name matching the active Claude instance, e.g.:

  ```
  Co-authored-by: Claude Sonnet 4.6 <noreply@anthropic.com>
  ```

  This applies to all commits under `cbsd-rs/`, including subprojects,
  documentation, and tooling (`.claude/skills/`, etc.).
- Separate `git add` and `git commit` commands (not chained)
- Ceph project commit message style

## Commit Granularity

Each commit should be the **smallest compilable, testable, logical unit** —
but never so small that it's meaningless in isolation.

- When a planned commit has naturally separable subsystems with clean
  dependency boundaries, **split at those boundaries** to reduce blast radius.
- When parts are tightly coupled (one doesn't work without the other),
  **keep them together** — splitting would create broken intermediate commits.
- Target ~400–800 authored lines per commit. Above 800, look for a natural
  split. Below 200, consider whether the commit is meaningful alone.
- A DB module + the route handlers consuming it = two commits if the DB
  module is independently testable. One commit if the handler is the only
  way to exercise the DB code.
- **The test:** Can someone reviewing this commit understand its purpose in
  one sentence? Can the previous commit compile and pass tests? Could this
  commit be reverted without breaking unrelated functionality?

## Design & Plans

- **Design documents (authoritative):** `cbsd-rs/docs/cbsd-rs/design/`
  - Architecture & task queue, auth & permissions, project structure
  - If code and design disagree, **fix the code**
- **Implementation plans:** `cbsd-rs/docs/cbsd-rs/plans/`
  - Phased commit plan with progress tracking tables
  - **Update plan progress tables after completing each commit**
  - See `cbsd-rs/docs/cbsd-rs/plans/README.md` for instructions
- **`cbc` docs:** `cbsd-rs/docs/cbc/`
- See `/cbsd-rs-docs` skill for file naming and directory conventions.

## Key Reference Files

- `cbsdcore/src/cbsdcore/versions.py` — Python BuildDescriptor (port target)
- `cbsd/cbslib/core/permissions.py` — Python permission model (reference)
- `cbsd/cbslib/auth/auth.py` — Python PASETO token handling (reference)

## Correctness Invariants

These are easy to get wrong. Document and test them:

1. **Dispatch mutex + SQLite write ordering:** The dispatch mutex is held
   across the SQLite write (QUEUED → DISPATCHED), then released before the
   WebSocket send. This prevents the crash gap where memory says DISPATCHED
   but the DB says QUEUED. The critical section includes SQLite I/O (~1-5ms),
   which is why `tokio::sync::Mutex` (async, yield-safe) is required, not
   `std::sync::Mutex`.

2. **SQLite pool sizing:** `max_connections = 4`. The dispatch mutex holds
   across a SQLite write. If the pool is exhausted, the sqlx query stalls
   while holding the mutex, and other queue operations that also need pool
   connections deadlock.

3. **`PRAGMA foreign_keys = ON`:** Must be set per-connection via
   `SqliteConnectOptions::pragma()`. SQLite does not enforce FK constraints
   or `ON DELETE CASCADE` by default. Without it, role/user deletions leave
   orphan rows and the last-admin guard produces wrong results.

4. **`trace_id` lifecycle:** Generated as UUID v4 at dispatch time, under
   the mutex. Persisted in `builds.trace_id`. Included in the `build_new`
   WebSocket message. Worker sets `CBS_TRACE_ID` env var for cbscore
   subprocess. Enables cross-boundary log correlation.

5. **`tower-sessions-sqlx-store` initialization:** Call `.migrate().await`
   after sqlx migrations but before the router is built. Creates the
   `tower_sessions` table (managed by the library, not cbsd migrations).

6. **Watch sender lifecycle:** `AppState.log_watchers: HashMap<BuildId,
   watch::Sender<()>>` — created at dispatch time, dropped by
   `build_finished` handler and by startup recovery. Stored separately from
   `ActiveBuild` to avoid coupling the queue struct to the log subsystem.

7. **SSE file descriptor lifetime:** The SSE handler opens the log file once
   at stream start and holds the FD for the entire stream duration. On Linux,
   an open FD to an unlinked file survives deletion. This prevents the GC
   race where the file is deleted between reads.

## sqlx Offline Query Cache

The `.sqlx/` directory lives at the workspace root (`cbsd-rs/.sqlx/`).


**After any commit that adds or modifies sqlx queries:**

1. Ensure a dev DB exists: `DATABASE_URL=sqlite:///tmp/cbsd-dev.db`
2. Run migrations: `cargo sqlx migrate run`
3. Prepare cache: `cargo sqlx prepare --workspace`
4. Verify: `SQLX_OFFLINE=true cargo build --workspace`
5. Include `.sqlx/` changes in the commit


**Bootstrap (first time):**

1. `cargo sqlx database create`
2. `cargo sqlx migrate run`
3. Write query code
4. `cargo sqlx prepare --workspace`
