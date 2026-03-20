# cbsd Rust Port — Project Structure & Crate Organization

## Overview

The cbsd-rs project is organized as a **Cargo workspace** with three crates.
The server and worker have very different dependency profiles — separating
them into distinct crates keeps compile times and binary sizes honest.

## Workspace Layout

```
cbsd-rs/
├── Cargo.toml                  # workspace root
├── Cargo.lock
├── migrations/                 # sqlx migrations (shared, embedded by server)
│   ├── 001_initial_schema.sql
│   └── ...
│
├── cbsd-proto/                 # shared types crate (library)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── ws.rs               # WebSocket protocol message types
│       ├── build.rs            # BuildDescriptor, BuildId, BuildState, Priority
│       ├── component.rs        # Component descriptor types
│       ├── config.rs           # Shared config structures (server URL, etc.)
│       └── arch.rs             # Arch enum (x86_64, aarch64)
│
├── cbsd-server/                # server binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # entry point, CLI args, app setup
│       ├── app.rs              # axum router construction, lifespan
│       ├── config.rs           # server-specific config (extends proto config)
│       ├── db/                 # SQLite via sqlx
│       │   ├── mod.rs
│       │   ├── users.rs
│       │   ├── tokens.rs
│       │   ├── api_keys.rs
│       │   ├── roles.rs
│       │   ├── builds.rs
│       │   └── migrations.rs   # sqlx::migrate!() setup
│       ├── auth/               # authentication + authorization
│       │   ├── mod.rs
│       │   ├── extractors.rs   # AuthUser, RequireCap, ownership checks
│       │   ├── oauth.rs        # Google SSO flow
│       │   ├── paseto.rs       # token create/decode
│       │   └── api_keys.rs     # API key hashing, LRU cache
│       ├── routes/             # axum route handlers
│       │   ├── mod.rs
│       │   ├── auth.rs         # /auth/* endpoints
│       │   ├── builds.rs       # /builds/* endpoints
│       │   ├── components.rs   # /components endpoint
│       │   ├── workers.rs      # /workers endpoint
│       │   ├── admin.rs        # /admin/* endpoints
│       │   └── permissions.rs  # /permissions/* endpoints
│       ├── queue/              # build queue + dispatch
│       │   ├── mod.rs
│       │   ├── dispatch.rs     # priority lanes, mutex, dispatch logic
│       │   └── recovery.rs     # startup recovery from SQLite
│       ├── ws/                 # WebSocket handler (server side)
│       │   ├── mod.rs
│       │   ├── handler.rs      # per-connection message loop
│       │   ├── liveness.rs     # grace period, reconnection, Stopping state
│       │   └── dispatch.rs     # build_new send, ack timer
│       ├── logs/               # build log management
│       │   ├── mod.rs
│       │   ├── writer.rs       # append to file, seq→offset index, watch notify
│       │   └── sse.rs          # SSE follow endpoint
│       └── components/         # component store
│           ├── mod.rs
│           └── tarball.rs      # pack component directory → tar.gz
│
└── cbsd-worker/                # worker binary
    ├── Cargo.toml
    └── src/
        ├── main.rs             # entry point, CLI args
        ├── config.rs           # worker-specific config
        ├── ws/                 # WebSocket client
        │   ├── mod.rs
        │   ├── connection.rs   # connect, reconnect loop, backoff
        │   └── handler.rs      # message dispatch (hello, build_new, etc.)
        ├── build/              # build execution
        │   ├── mod.rs
        │   ├── executor.rs     # subprocess management, process group
        │   ├── output.rs       # line buffering, seq tracking, batching
        │   └── component.rs    # unpack tarball, validate SHA-256
        └── signal.rs           # SIGTERM handling, graceful shutdown
```

## Crate Dependencies

### `cbsd-proto` (shared types)

Minimal dependencies — only serialization and time:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
```

This crate defines all types that cross the WebSocket boundary. Both server
and worker depend on it, ensuring compile-time agreement on the wire format.
Changes to `cbsd-proto` trigger recompilation of both binaries.

**What goes here:**

- All WebSocket message types (serde-tagged enums)
- `BuildDescriptor`, `BuildId`, `BuildState`, `Priority`
- `Arch` enum (validated set: `x86_64`, `aarch64` with `arm64` alias)
  Use `#[serde(rename = "aarch64", alias = "arm64")]` on the `Aarch64` variant.
- Component descriptor types
- Shared config structures (server URL, TLS settings)

**What does NOT go here:**

- Server-specific logic (routes, DB, queue, auth)
- Worker-specific logic (subprocess, reconnection)
- Any IO or async runtime dependency

### `cbsd-server`

Heavy on web framework, database, and auth:

```toml
[dependencies]
cbsd-proto = { path = "../cbsd-proto" }
axum = { version = "0.7", features = ["ws"] }
axum-extra = { version = "0.9", features = ["typed-header"] }
axum-server = { version = "0.6", features = ["tls-rustls"] }
tower = "0.4"
tower-sessions = "0.13"
tower-sessions-sqlx-store = { version = "0.13", features = ["sqlite"] }
tower-governor = "0.4"
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
reqwest = { version = "0.12", features = ["json"] }
pasetors = "0.6"
argon2 = "0.5"
sha2 = "0.10"
lru = "0.12"
hkdf = "0.12"                   # session key derivation (HKDF-SHA256)
uuid = { version = "1", features = ["v4"] }  # trace_id, connection_id
clap = { version = "4", features = ["derive"] }  # CLI args, --drain flag
tokio-util = "0.7"              # CancellationToken for ack timers
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yml = "0.0.12"
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
glob-match = "0.2"
```

### `cbsd-worker`

Lightweight — WebSocket client, subprocess management, no web framework:

```toml
[dependencies]
cbsd-proto = { path = "../cbsd-proto" }
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-native-roots"] }
http = "1"              # for Authorization header on WS upgrade (no reqwest needed)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yml = "0.0.12"
sha2 = "0.10"
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
flate2 = "1"           # tar.gz decompression
tar = "0.4"            # tarball extraction
nix = { version = "0.29", features = ["signal", "process"] }  # process group mgmt
```

## Build & Run

```bash
# Build everything
cargo build --release

# Server binary
./target/release/cbsd-server --config server.yaml

# Worker binary
./target/release/cbsd-worker --config worker.yaml
```

## SQLite Migrations

Migrations use `sqlx::migrate!()` with embedded `.sql` files in the
`migrations/` directory at the workspace root. The server runs migrations
at startup before accepting connections.

```rust
// In cbsd-server/src/db/migrations.rs
sqlx::migrate!("../migrations")
    .run(&pool)
    .await?;
```

Migration files are numbered sequentially (`001_`, `002_`, ...) and are
append-only. Each migration runs in a transaction. Backward-incompatible
schema changes (column type changes, column removal) require a coordinated
deploy: new migration + new server version + new cbc version.

The `migrations/` directory lives at the workspace root (not inside
`cbsd-server/`) so that the offline `sqlx` CLI tooling (`sqlx database
create`, `sqlx migrate run`) works without navigating into a subcrate.

**sqlx offline query cache:** Compile-time checked queries require either a
live database connection or an offline query cache (`.sqlx/` directory or
`sqlx-data.json`). The cache is generated by `cargo sqlx prepare` and must be
committed to the repository. CI builds use `SQLX_OFFLINE=true` to build
without a live database. Regenerate the cache after any migration or query
change.

## Container Images

Two container images are produced:

- **`cbsd-server`**: Contains the server binary, migrations, and
  `components/` directory (mounted or baked in).
- **`cbsd-worker`**: Contains the worker binary and a Python environment
  with `cbscore` installed (for the subprocess bridge).

Both images are minimal (distroless or alpine-based) with only the required
runtime dependencies. The server image does not include Python; the worker
image does not include `sqlx` or `axum`.
