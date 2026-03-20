# cbsd-rs — CBS Build Service Daemon (Rust)

Rust rewrite of `cbsd/`. Replaces the FastAPI + Celery + Redis stack with
axum + SQLite + WebSocket. Three binaries:

- **cbsd-server** — REST API, build queue, periodic build scheduler,
  SSE log streaming, WebSocket endpoint for workers, Google OAuth + RBAC
- **cbsd-worker** — WebSocket client, cbscore build subprocess bridge
- **cbc** — CLI client for the build service (login, submit builds, stream
  logs, admin operations)

## Building

The project uses [sqlx](https://github.com/launchbadge/sqlx) compile-time
checked SQL queries. The offline query cache is committed at `cbsd-rs/.sqlx/`,
so no database is needed for normal builds.

**Normal build** (uses the committed `.sqlx/` cache):

```bash
SQLX_OFFLINE=true cargo build --workspace
```

**When modifying queries or migrations** (adds or changes a `query!()` call,
or touches `migrations/`), regenerate the cache before committing:

```bash
export DATABASE_URL=sqlite:///tmp/cbsd-dev.db
cargo sqlx database create
cargo sqlx migrate run
cargo sqlx prepare --workspace
SQLX_OFFLINE=true cargo build --workspace   # verify
git add .sqlx/
```

From the repository root, `./do-cbsd-rs-compose.sh sqlx-prepare` automates
the database setup and migration steps.

## Development

### Prerequisites

- **Rust toolchain** — install via [rustup](https://rustup.rs/). The
  workspace uses the 2024 edition.
- **Podman** and **podman-compose** — the dev environment runs server and
  worker in containers.
- **cargo-sqlx** — only needed when adding or modifying `query!()` macros
  or SQL migrations. Install with:
  ```bash
  cargo install sqlx-cli --no-default-features --features sqlite
  ```
- **A cbscore config file** (`cbs-build.config.yaml`) — the worker container
  needs this to run builds. Use an existing one from your host or create a
  minimal placeholder.
- **A Google OAuth secrets JSON** — the `prepare` script requires this file,
  but in dev mode its contents are ignored (the server bypasses OAuth). An
  empty JSON object `{}` saved to a file is sufficient.

### Bringing up the dev environment

All commands below are run from the **repository root** (not `cbsd-rs/`).

1. **Prepare configuration** — generates server and worker YAML configs
   under `_local/cbsd-rs/`:

   ```bash
   ./do-cbsd-rs-compose.sh prepare --dev \
       --google-client-secrets ~/client_secret.json \
       --cbscore-config ~/cbs-build.config.yaml \
       --seed-admin you@example.com
   ```

   The `--dev` flag enables two things in the generated config:
   - **Dev mode** (`dev.enabled: true`) — the server bypasses Google OAuth
     and auto-authenticates as the seed admin on login.
   - **Seed workers** — a worker with a pre-shared API key is seeded into
     the database at startup, so no manual registration is required.

2. **Start the stack**:

   ```bash
   ./do-cbsd-rs-compose.sh up --dev
   ```

   This builds and starts two containers (`server-dev`, `worker-dev`) with
   cargo-watch. Rust source changes on the host trigger an incremental
   rebuild and restart inside the container.

3. **Verify**:

   ```bash
   # Login (returns a PASETO token — dev mode, no browser needed)
   curl -s http://localhost:8080/api/auth/login?client=cli

   # Or use the cbc CLI
   cd cbsd-rs && cargo run --bin cbc -- login http://localhost:8080
   ```

### Dev mode config options

The `prepare --dev` script generates a `server.yaml` with the following
dev-specific section (see `config/server.yaml.example` for full reference):

```yaml
dev:
  enabled: true
  seed-workers:
    - name: dev-worker-x86
      arch: x86_64
      api-key: "cbsk_<generated-hex>"
```

| Option | Effect |
|--------|--------|
| `dev.enabled` | Bypasses Google OAuth — login returns a token for the seed admin. Skips OAuth config validation at startup. |
| `dev.seed-workers` | Pre-registers workers with the given API keys at startup. The worker config uses the matching `api-key` to connect without a registration token. |

The worker's `worker.yaml` is generated with the matching `api-key` and
`arch` so the two containers connect automatically.

### Logging

Application logging is controlled by two mechanisms:

| Mechanism | Controls | Scope |
|-----------|----------|-------|
| `CBSD_DEV=1` env var | Console (stdout) output | Both server and worker |
| `logging.log-file` in YAML config | File output | Per-binary |

**Development mode** (`CBSD_DEV=1`): Console output is always enabled.
File output is additionally enabled when `logging.log-file` is configured.
The compose `--dev` profile sets `CBSD_DEV=1` automatically.

**Production mode** (no `CBSD_DEV`): Console output is disabled. The
`logging.log-file` config is **required** — the binary refuses to start
without it.

`CBSD_DEV` is independent from `dev.enabled` in `server.yaml`. The server
config `dev.enabled` controls OAuth bypass and worker seeding. Setting
`dev.enabled: true` without `CBSD_DEV=1` gives OAuth bypass but file-only
logging. The compose `--dev` flag sets both.

Log rotation and compression for production deployments is handled by
`logrotate` on the host (see the systemd deployment section).

### Useful commands

```bash
# Rebuild images from scratch (after Cargo.toml or dependency changes)
./do-cbsd-rs-compose.sh up --dev --rebuild

# Stop the stack
./do-cbsd-rs-compose.sh down --dev

# Regenerate sqlx offline cache (after query or migration changes)
./do-cbsd-rs-compose.sh sqlx-prepare
```

## Configuration

All YAML configuration keys use **kebab-case** throughout (e.g. `listen-addr`,
`token-secret-key`, `seed-admin`, `server-url`). Annotated examples:

- `cbsd-rs/config/server.yaml.example`
- `cbsd-rs/config/worker.yaml.example`

## Compose deployment (development / staging)

The `podman-compose.cbsd-rs.yaml` file is primarily intended for **development
and staging**. It builds images locally from source and bind-mounts
configuration from `_local/cbsd-rs/`. For production, use the systemd-based
setup instead (see [Production deployment](#production-deployment-systemd)
below).

Two compose profiles are available:

| Profile | Command flag | Description |
|---------|-------------|-------------|
| `dev`   | `--dev`     | cargo-watch auto-reload; source bind-mounted at `/cbs/src` |
| `prod`  | _(default)_ | pre-built local binaries; suitable for staging |

The `do-cbsd-rs-compose.sh` helper script automates config generation and
image management. It requires two external files — a Google OAuth2 client
secrets JSON and a cbscore `cbs-build.config.yaml` — that are not committed
to the repository.

**Dev quickstart** (pre-configured worker API key, OAuth bypass — no Google
credentials or REST API calls needed):

```bash
./do-cbsd-rs-compose.sh prepare --dev \
    --google-client-secrets ~/client_secret.json \
    --cbscore-config ~/cbs-build.config.yaml \
    --seed-admin you@example.com
./do-cbsd-rs-compose.sh up --dev
```

In dev mode the server bypasses Google OAuth entirely — the login endpoint
returns a token for the seed admin without contacting Google. The
`--google-client-secrets` file is still required by the helper script but
its contents are not used.

**Staging / prod-like compose** (workers registered via REST API after first
server start):

```bash
./do-cbsd-rs-compose.sh prepare \
    --google-client-secrets ~/client_secret.json \
    --cbscore-config ~/cbs-build.config.yaml \
    --seed-admin admin@example.com
./do-cbsd-rs-compose.sh up
# Follow the printed instructions to register a worker via the REST API.
```

Run `./do-cbsd-rs-compose.sh --help` for full options including
`--worker-name`, `--arch`, `--allowed-domain`, and `--rebuild`.

## Production deployment (systemd)

For production, use the systemd user-service installer in
`cbsd-rs/systemd/`. It pulls pre-built images from the registry and
integrates with the host service manager rather than managing the
container lifecycle through podman-compose.

```bash
# Install server + worker services for the default deployment
./cbsd-rs/systemd/install.sh

# Install server only, or a named worker instance
./cbsd-rs/systemd/install.sh server
./cbsd-rs/systemd/install.sh worker --name host-01
```

The installer places systemd unit files under `~/.config/systemd/user/`
and prints per-service instructions explaining what config files must be
created before the service can be started.

**Worker registration** — workers must be registered via the server REST
API (not pre-seeded at startup). After the server is running and you have
an admin token, register each worker and copy the returned `worker-token`
into the worker's `worker.yaml`:

```bash
curl -X POST http://<server-host>:8080/api/admin/workers \
  -H "Authorization: Bearer <admin-token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "worker-x86-01", "arch": "x86_64"}'
# response includes: {"worker-token": "..."}
```
