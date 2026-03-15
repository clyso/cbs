# Phase 1 — Foundation: Workspace, Shared Types, Schema, Server Scaffold

## Progress

| Item | Status |
|------|--------|
| Commit 1: cbsd-proto crate with all shared types | Done |
| Commit 2: Schema, server scaffold, config loading | Done |

## Goal

A compiling Cargo workspace with all shared types, SQLite schema migrated, and
a server binary that boots, connects to the database, and serves a health
endpoint.

## Commit 1: cbsd-proto crate with all shared types

The foundation crate that both server and worker depend on. Zero IO
dependencies — only `serde`, `serde_json`, `chrono`.

**Workspace root** (`cbsd-rs/Cargo.toml`):
- Workspace members: `cbsd-proto`, `cbsd-server`, `cbsd-worker`
- Shared dependency versions in `[workspace.dependencies]`

**cbsd-proto contents:**
- `build.rs` (note: source file, not Cargo build script) — `BuildDescriptor` (preserving Python nesting: `version`,
  `channel`, `version_type`, `signed_off_by`, `dst_image`, `components[]`,
  `build` with nested `BuildTarget` containing `distro`, `os_version`,
  `artifact_type`, `arch`). `BuildState` enum (7 states). `Priority` enum.
  `BuildId` newtype.
- `arch.rs` — `Arch` enum with `x86_64` and `aarch64` (`arm64` alias via
  `#[serde(alias)]`).
- `ws.rs` — Server→Worker and Worker→Server message enums (serde-tagged):
  `build_new`, `build_revoke`, `welcome` (includes `protocol_version`,
  `connection_id`, and `grace_period_secs` — worker validates its backoff
  ceiling against this value), `error` (includes `min_version`,
  `max_version`), `hello`, `worker_status`, `build_accepted`,
  `build_rejected`, `build_started`, `build_output` (with `start_seq`),
  `build_finished`, `worker_stopping`.
- `config.rs` — Shared config types (server URL, TLS CA bundle path).
- `lib.rs` — Re-exports.

**Stub crates** (minimal, just enough to compile the workspace):
- `cbsd-server/src/main.rs` — `fn main() { println!("server"); }`
- `cbsd-worker/src/main.rs` — `fn main() { println!("worker"); }`

**Testable:** `cargo build` succeeds. `cargo test -p cbsd-proto` passes serde
round-trip tests on BuildDescriptor, WS messages, Arch alias.

## Commit 2: Schema, server scaffold, config loading

The server binary boots, creates the SQLite database, and responds to a
health check.

**Migrations** (`cbsd-rs/migrations/001_initial_schema.sql`):
- All 8 tables: `users`, `tokens`, `api_keys`, `roles`, `role_caps`,
  `user_roles`, `user_role_scopes`, `builds`, `build_logs`
- `builds` table includes `descriptor_version INTEGER NOT NULL DEFAULT 1`
  and `trace_id TEXT` (nullable — NULL for QUEUED builds, set at dispatch)
- `api_keys` table includes `UNIQUE(name, owner_email)` constraint
- All 4 indexes: `idx_tokens_user`, `idx_builds_state`, `idx_builds_user`,
  `idx_builds_state_queued`
- CHECK constraints on `state`, `priority`, `scope_type`

**Server scaffold:**
- `main.rs` — CLI args (config path), tokio runtime entry point
- `config.rs` — `ServerConfig` with serde_yml deserialization. All config
  fields from the design doc (listen_addr, TLS, DB, log_dir, secrets, OAuth,
  rate limiting, timeouts, seeding). **Config validation at startup:** panic
  if `allowed_domains` is empty and `allow_any_google_account` is not `true`;
  panic if `reconnect_backoff_ceiling_secs >= liveness_grace_period_secs`.
- `app.rs` defines `AppState` as the canonical shared state struct.
  Subsequent commits extend this struct with queue, cache, and WS handles.
- `db/mod.rs` — `create_pool()`: `SqlitePool` with pragmas (WAL,
  `foreign_keys=ON`, `busy_timeout=5000`, `synchronous=NORMAL`),
  `max_connections=4`.
- `db/migrations.rs` — `sqlx::migrate!("../migrations")` wrapper.
- `app.rs` — axum `Router` with a single `GET /api/health` returning 200.
  Tracing subscriber setup (console + optional file appender via
  `tracing-appender`). Lifespan: open DB pool → run sqlx migrations →
  construct `SqliteStore` from pool + call `.migrate().await` (creates
  `tower_sessions` table, managed by library not cbsd migrations) → wire
  into router via `CookieManagerLayer` + `SessionManagerLayer` → build
  router → serve.

**sqlx offline cache bootstrap procedure:**
1. Write migration SQL files first.
2. Set `DATABASE_URL=sqlite:///tmp/cbsd-dev.db` in environment.
3. `cargo sqlx database create` + `cargo sqlx migrate run` to create dev DB.
4. Write all `db/*.rs` query code with `DATABASE_URL` pointing to the live
   dev DB (sqlx macros compile against it).
5. `cargo sqlx prepare --workspace` from workspace root — writes
   `cbsd-rs/.sqlx/` directory (workspace root, not per-crate).
6. Verify: `SQLX_OFFLINE=true cargo build --workspace` succeeds.
7. Commit `.sqlx/` directory. Re-run `cargo sqlx prepare` after any
   migration or query change.

**Testable:** Server binary boots, creates `cbsd.db`, runs migrations,
responds to `GET /api/health`. Config loading validates and rejects malformed
YAML. Config panics if `allowed_domains` empty without
`allow_any_google_account: true`. Config panics if
`reconnect_backoff_ceiling_secs >= liveness_grace_period_secs`. Persisted
build records have `descriptor_version=1`. `SQLX_OFFLINE=true cargo build`
succeeds.
