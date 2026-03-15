# cbsd-rs Deployment Plan

## Overview

This document describes how to build, deploy, and operate the Rust
reimplementation of the CBS build service daemon (`cbsd-rs`). It covers local
development, container image production, systemd-managed production
deployments, and the migration path from the Python `cbsd` stack.

The Rust port eliminates three runtime dependencies:

| Python stack | Rust replacement |
|---|---|
| Celery worker | WebSocket client (`cbsd-worker`) |
| Redis (broker + result backend + log streams) | Eliminated entirely |
| `dbm` files + `permissions.yaml` | Single SQLite database (`cbsd.db`) |

The server and worker are both static Rust binaries. The worker additionally
needs a Python environment with `cbscore` installed (it spawns cbscore as a
subprocess).

---

## 1. Development Setup

### 1.1 Prerequisites

- Rust toolchain (stable, 2024 edition) via `rustup`
- `cargo-sqlx` CLI: `cargo install sqlx-cli --features sqlite`
- Python 3.13+ and `uv` (for cbscore on the worker side)
- SQLite 3.35+ (system package; used by sqlx at build time)
- OpenSSL CLI (for generating self-signed TLS certs)

### 1.2 Building

From the workspace root (`cbsd-rs/`):

```bash
cd cbsd-rs/

# Build all crates
cargo build --workspace

# Release build
cargo build --workspace --release
```

Binaries land at `cbsd-rs/target/{debug,release}/{cbsd-server,cbsd-worker}`.

### 1.3 Running the server locally

**First-time database setup:**

```bash
# Create the SQLite database and run migrations
export DATABASE_URL=sqlite:///tmp/cbsd-dev.db
cargo sqlx database create
cargo sqlx migrate run --source migrations/
```

**Generate a self-signed TLS cert (for local dev):**

```bash
mkdir -p _local/cbsd-rs/config
openssl req -x509 -newkey rsa:4096 \
  -keyout _local/cbsd-rs/config/cbs.key.pem \
  -out _local/cbsd-rs/config/cbs.cert.pem \
  -days 365 -nodes -subj "/CN=localhost"
```

**Create a minimal server config** at `_local/cbsd-rs/config/server.yaml`:

```yaml
# cbsd-rs server config (development)
listen_addr: "0.0.0.0:8080"

database_path: "/tmp/cbsd-dev.db"

tls:
  cert_path: "_local/cbsd-rs/config/cbs.cert.pem"
  key_path: "_local/cbsd-rs/config/cbs.key.pem"

log_dir: "_local/cbsd-rs/logs"
log_retention_days: 30

components_dir: "components"

oauth:
  client_secrets_file: "_local/cbsd-rs/config/google-client-cbs.json"
  allowed_domains:
    - "clyso.com"

secrets:
  session_secret_key: "$(openssl rand -hex 32)"
  token_secret_key: "$(openssl rand -hex 32)"

max_token_ttl_seconds: none  # infinite

# First-startup bootstrapping
seed_admin: "admin@clyso.com"
seed_worker_api_keys:
  - name: "dev-worker-01"

rate_limit:
  auth_requests_per_minute: 10

# Worker liveness
worker_grace_period_secs: 90
dispatch_ack_timeout_secs: 15
```

**Start the server:**

```bash
cargo run --bin cbsd-server -- --config _local/cbsd-rs/config/server.yaml
```

On first startup the server will:

1. Run SQLite migrations.
2. Seed builtin roles (`admin`, `builder`, `viewer`).
3. Create the seed admin user.
4. Generate and print worker API key(s) to stdout (save these).

### 1.4 Running the worker locally

**Install cbscore** (needed for the subprocess bridge):

```bash
# From the repo root
uv sync --package cbscore
```

**Create a minimal worker config** at `_local/cbsd-rs/config/worker.yaml`:

```yaml
# cbsd-rs worker config (development)
server_url: "wss://localhost:8080/api/ws/worker"
api_key: "cbsk_<paste-key-from-server-stdout>"
worker_id: "dev-local-01"
arch: "x86_64"
build_timeout_secs: 7200  # 2 hours
component_temp_dir: "/tmp/cbsd-worker-components"

# cbscore Python wrapper
cbscore_wrapper_path: "cbsd-rs/scripts/cbscore-wrapper.py"
cbscore_config_path: "_local/cbs/config/worker/cbscore.config.yaml"

# TLS: trust self-signed cert from dev server
tls_ca_bundle_path: "_local/cbsd-rs/config/cbs.cert.pem"

# Reconnection
reconnect_initial_secs: 1
reconnect_ceiling_secs: 30
```

**Start the worker:**

```bash
cargo run --bin cbsd-worker -- --config _local/cbsd-rs/config/worker.yaml
```

The worker connects to the server over WebSocket, sends `hello`, receives
`welcome`, and enters the idle state.

### 1.5 Mapping to the existing dev workflow

The current Python dev workflow uses `do-cbs-compose.sh` with
`podman-compose.cbs.yaml` (or `podman-compose.cbs-dev.yaml`), which starts
three containers: `cbs` (server), `worker`, and `redis`.

For cbsd-rs development, the simplest approach is to run the server and worker
directly on the host (no containers, no compose) using `cargo run`. This is
faster for iteration and avoids container image rebuilds.

For integration testing with containers, a new compose file is needed (see
section 3.2). The existing `podman-compose.cbs.yaml` is Python-specific and
should not be modified -- cbsd-rs gets its own compose file.

---

## 2. Container Images

### 2.1 Image design

Two images, matching the current naming convention:

| Image | Contents | Base |
|---|---|---|
| `cbsd-rs-server` | Rust server binary + migrations + `components/` | `gcr.io/distroless/cc-debian12` or `alpine:3.20` |
| `cbsd-rs-worker` | Rust worker binary + Python 3.13 + cbscore + `cbscore-wrapper.py` | `python:3.13-alpine3.20` |

The server image does not contain Python. The worker image does not contain
SQLite CLI or axum -- only the worker binary and the Python runtime.

### 2.2 Dockerfile locations

New files, parallel to the existing `container/ContainerFile.cbsd`:

```
container/
  ContainerFile.cbsd              # existing Python images (unchanged)
  ContainerFile.cbsd-rs           # new: Rust server + worker images
  entrypoint.sh                   # existing Python server entrypoint
  entrypoint-cbsd-rs-server.sh    # new: Rust server entrypoint
  build.sh                        # existing build script (unchanged)
  build-rs.sh                     # new: build script for Rust images
```

### 2.3 ContainerFile.cbsd-rs

```dockerfile
# --- Build stage: compile Rust binaries ---
FROM docker.io/rust:1-alpine AS rust-builder

RUN apk add --no-cache musl-dev pkgconfig openssl-dev

WORKDIR /build
COPY cbsd-rs/ ./cbsd-rs/

WORKDIR /build/cbsd-rs
ENV SQLX_OFFLINE=true
RUN cargo build --release --workspace

# --- Server image ---
FROM docker.io/alpine:3.20 AS cbsd-rs-server

RUN apk add --no-cache ca-certificates sqlite-libs

COPY --from=rust-builder /build/cbsd-rs/target/release/cbsd-server /usr/local/bin/cbsd-server
COPY container/entrypoint-cbsd-rs-server.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Components directory (can be overridden by volume mount)
COPY components/ /cbs/components/

VOLUME ["/cbs/config", "/cbs/data", "/cbs/logs"]
EXPOSE 8080

ENTRYPOINT ["/entrypoint.sh"]

# --- Worker image ---
FROM docker.io/python:3.13-alpine3.20 AS cbsd-rs-worker

RUN apk add --no-cache podman=5.2.5-r0 ca-certificates bash

# Install uv
RUN curl -LsSf https://astral.sh/uv/install.sh | \
  UV_INSTALL_DIR=/usr/bin \
  UV_DISABLE_UPDATE=1 \
  UV_NO_MODIFY_PATH=1 \
  /bin/sh

# Install cbscore
WORKDIR /cbs
COPY pyproject.toml ./
COPY cbscore/ ./cbscore/
COPY cbsdcore/ ./cbsdcore/
RUN uv add --workspace --no-cache ./cbscore && \
    uv add --workspace --no-cache ./cbsdcore && \
    uv sync --all-packages --no-cache --no-dev

# Copy worker binary and wrapper script
COPY --from=rust-builder /build/cbsd-rs/target/release/cbsd-worker /usr/local/bin/cbsd-worker
COPY cbsd-rs/scripts/cbscore-wrapper.py /cbs/scripts/cbscore-wrapper.py

VOLUME ["/cbs/config", "/cbs/scratch", "/var/lib/containers", "/cbs/ccache"]

ENTRYPOINT ["/usr/local/bin/cbsd-worker", "--config", "/cbs/config/worker.yaml"]
```

### 2.4 Server entrypoint script

New file: `container/entrypoint-cbsd-rs-server.sh`

```bash
#!/bin/bash
set -euo pipefail

config_dir="${CBSD_RS_CONFIG:-/cbs/config}"

[[ ! -d "${config_dir}" ]] && {
  echo "error: config dir at '${config_dir}' does not exist" >&2
  exit 1
}

exec /usr/local/bin/cbsd-server --config "${config_dir}/server.yaml"
```

### 2.5 Build script

New file: `container/build-rs.sh`

This mirrors `container/build.sh` but targets the Rust images:

```bash
#!/bin/bash
set -euo pipefail

[[ ! -e ".git" ]] &&
  echo "error: must be run from repository root" >/dev/stderr && exit 1

repo_tag="$(git describe --always 2>/dev/null)"
registry="${REGISTRY:-harbor.clyso.com}"
server_image="cbs/cbsd-rs-server:${repo_tag}"
worker_image="cbs/cbsd-rs-worker:${repo_tag}"

echo "Building cbsd-rs server: ${registry}/${server_image}"
podman build -f ./container/ContainerFile.cbsd-rs \
  --target cbsd-rs-server \
  --tag "${registry}/${server_image}" .

echo "Building cbsd-rs worker: ${registry}/${worker_image}"
podman build -f ./container/ContainerFile.cbsd-rs \
  --target cbsd-rs-worker \
  --tag "${registry}/${worker_image}" .
```

### 2.6 Build commands (summary)

```bash
# From repo root:
./container/build-rs.sh

# Or manually:
podman build -f container/ContainerFile.cbsd-rs --target cbsd-rs-server -t cbsd-rs-server:dev .
podman build -f container/ContainerFile.cbsd-rs --target cbsd-rs-worker -t cbsd-rs-worker:dev .
```

---

## 3. Production Deployment

### 3.1 What changes from the Python stack

| Python stack component | Rust replacement | Action |
|---|---|---|
| `clyso-cbsd-server` container (FastAPI + Uvicorn) | `cbsd-rs-server` container (axum) | Replace |
| `clyso-cbsd-worker` container (Celery) | `cbsd-rs-worker` container (WS client) | Replace |
| `cbs-redis` container (Redis 8.4) | Nothing | Remove entirely |
| `_local/cbs/redis/` data directory | Nothing | Remove after migration |
| `_local/cbs/data/db/` (dbm files) | `_local/cbs/data/cbsd.db` (SQLite) | One-time migration |
| `permissions.yaml` (static YAML) | SQLite `roles` + `user_role_scopes` tables | One-time migration script |
| Celery health check (`celery inspect ping`) | WebSocket connectivity (server `/api/workers` endpoint) | Update health checks |

### 3.2 Podman Compose (development/staging)

New file: `podman-compose.cbsd-rs.yaml`

```yaml
services:
  server:
    build:
      context: .
      dockerfile: ./container/ContainerFile.cbsd-rs
      target: cbsd-rs-server
    container_name: clyso-cbsd-rs-server
    ports:
      - "8080:8080"
    restart: "no"
    volumes:
      - ./_local/cbsd-rs/config/server:/cbs/config
      - ./_local/cbsd-rs/data:/cbs/data
      - ./_local/cbsd-rs/logs:/cbs/logs
      - ./components:/cbs/components
    security_opt:
      - label=disable
    environment:
      CBSD_RS_CONFIG: /cbs/config

  worker:
    build:
      context: .
      dockerfile: ./container/ContainerFile.cbsd-rs
      target: cbsd-rs-worker
    container_name: clyso-cbsd-rs-worker
    privileged: true
    volumes:
      - ./_local/cbsd-rs/config/worker:/cbs/config
      - ./_local/cbsd-rs/logs:/cbs/logs
      - ./_local/cbs/scratch:/cbs/scratch
      - ./_local/cbs/scratch/containers:/var/lib/containers
      - ./_local/cbs/scratch/ccache:/cbs/ccache
      - /dev/fuse:/dev/fuse:rw
    cap_add:
      - MKNOD
      - SYS_ADMIN
    security_opt:
      - label=disable
      - seccomp=unconfined
    # Host network required for VPN access during builds
    network_mode: host
    depends_on:
      - server
```

Key difference from the Python compose: **no redis service**. The compose file
has two services instead of three.

**Usage** (integrate into `do-cbs-compose.sh` or use directly):

```bash
PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose \
  -f podman-compose.cbsd-rs.yaml up --build
```

### 3.3 Systemd units (production)

The existing systemd templates in `systemd/templates/systemd/` use a
deployment-parameterized model (`%i` / `{{deployment}}`). The Rust port
follows the same pattern but does not need a network unit for Redis.

**New files in `systemd/templates/systemd/`:**

#### `cbsd-rs-server@.service`

```ini
[Unit]
Description=CBS Rust Server for deployment '%i'
After=network-online.target local-fs.target
Wants=network-online.target local-fs.target
PartOf=cbsd-%i.target
Before=cbsd-%i.target

[Service]
ExecStart=podman run --rm --name cbsd-rs-server-%i \
  -v /etc/cbsd/%i/server:/cbs/config:ro \
  -v /var/lib/cbsd/%i/data:/cbs/data \
  -v /var/log/cbsd/%i:/cbs/logs \
  -v /etc/cbsd/%i/components:/cbs/components:ro \
  -p 8080:8080 \
  --security-opt label=disable \
  harbor.clyso.com/cbs/cbsd-rs-server:latest
ExecStop=podman stop cbsd-rs-server-%i
Restart=on-failure
RestartSec=10s
Type=simple
TimeoutStopSec=120

[Install]
WantedBy=cbsd-%i.target
```

#### `cbsd-rs-worker@.service`

```ini
[Unit]
Description=CBS Rust Worker for deployment '%i'
After=network-online.target local-fs.target cbsd-rs-server@%i.service
Wants=network-online.target local-fs.target
PartOf=cbsd-%i.target
Before=cbsd-%i.target

[Service]
ExecStart=podman run --rm --name cbsd-rs-worker-%i \
  --privileged \
  --network host \
  --security-opt label=disable \
  --security-opt seccomp=unconfined \
  -v /etc/cbsd/%i/worker:/cbs/config:ro \
  -v /var/log/cbsd/%i:/cbs/logs \
  -v /var/lib/cbsd/%i/scratch:/cbs/scratch \
  -v /var/lib/cbsd/%i/scratch/containers:/var/lib/containers \
  -v /var/lib/cbsd/%i/scratch/ccache:/cbs/ccache \
  -v /dev/fuse:/dev/fuse:rw \
  --cap-add MKNOD --cap-add SYS_ADMIN \
  harbor.clyso.com/cbs/cbsd-rs-worker:latest
ExecStop=podman stop cbsd-rs-worker-%i
Restart=on-failure
RestartSec=10s
Type=simple
TimeoutStopSec=120

[Install]
WantedBy=cbsd-%i.target
```

Note: the `cbsd-network@.service` unit (which creates a podman network for
Redis connectivity) is **not needed** for cbsd-rs. The worker uses host
networking and connects to the server via its public address.

### 3.4 Config file management

Production deployments need two config files plus supporting secrets:

```
/etc/cbsd/<deployment>/
  server/
    server.yaml              # server config
    google-client-cbs.json   # Google OAuth client secrets
    cbs.cert.pem             # TLS certificate
    cbs.key.pem              # TLS private key
  worker/
    worker.yaml              # worker config
    cbscore.config.yaml      # cbscore paths, vault, secrets
    cbs.vault.yaml           # HashiCorp Vault credentials
    secrets.yaml             # build signing secrets
```

The server config (`server.yaml`) replaces three current files:
`cbsd.server.config.yaml`, `permissions.yaml`, and the Redis connection
strings. Permissions are now in SQLite, and Redis is gone.

The worker config (`worker.yaml`) replaces `cbsd.worker.config.yaml`. It no
longer contains Redis connection strings. Instead it has the server WebSocket
URL and an API key.

### 3.5 Secret management

| Secret | Where configured | Notes |
|---|---|---|
| PASETO token secret key | `server.yaml` → `secrets.token_secret_key` | 32-byte hex string. Generate with `openssl rand -hex 32`. |
| Session secret key | `server.yaml` → `secrets.session_secret_key` | 32-byte hex string. Used for HKDF derivation of session encryption keys. |
| Google OAuth client secrets | `server.yaml` → `oauth.client_secrets_file` | JSON file from Google Cloud Console. |
| TLS certificate + key | `server.yaml` → `tls.cert_path`, `tls.key_path` | PEM files. Self-signed for dev; CA-signed for production. |
| Worker API keys | Generated at first startup, printed to stdout | Save and distribute to worker configs. Store securely. |
| Worker API key (per worker) | `worker.yaml` → `api_key` | The `cbsk_...` string from server bootstrap output. |
| Vault credentials | `cbscore.config.yaml` → vault section | Worker-local. Same as current Python deployment. |
| Build signing secrets | `secrets.yaml` on worker | Worker-local. Same as current Python deployment. |

**Important:** The PASETO token secret key must be the same across server
restarts (it is used to decrypt tokens). If it changes, all existing tokens
become invalid. Back up `server.yaml` or manage via a secrets manager.

### 3.6 SQLite database

**Location:** Configured via `database_path` in `server.yaml`. Production
recommendation: `/var/lib/cbsd/<deployment>/data/cbsd.db`.

**Backup:** SQLite in WAL mode supports online backup. Use `sqlite3 cbsd.db
".backup /path/to/backup.db"` or filesystem snapshots. The database is small
(users, tokens, roles, build metadata) -- typically under 100 MB even with
thousands of builds.

**Pragmas** (set automatically by the server at connection time):

- `journal_mode = WAL`
- `foreign_keys = ON`
- `busy_timeout = 5000`
- `synchronous = NORMAL`

**Pool sizing:** `max_connections = 4` (correctness requirement to prevent
deadlock with the dispatch mutex; see design docs).

### 3.7 Log directory

Build logs are written to `{log_dir}/builds/{build_id}.log`. Server
application logs go to `{log_dir}/cbsd-server.log`.

```bash
mkdir -p /var/log/cbsd/<deployment>/builds
```

Log GC runs daily, deleting build log files older than `log_retention_days`
(default 30). Build metadata rows in SQLite are retained permanently.

### 3.8 TLS certificates

The server uses `rustls` (pure Rust, no OpenSSL runtime). TLS is configured
via `tls.cert_path` and `tls.key_path` in the server config. PEM format.

For production: use certificates from a CA (e.g., Let's Encrypt, or
internal PKI). The `do-cbs-compose.sh` script's `gen_server_keys()` function
generates self-signed certs and can be adapted for cbsd-rs development.

Workers connect via `wss://` and validate the server certificate against the
OS trust store by default. For private CAs or self-signed certs, set
`tls_ca_bundle_path` in the worker config to point to the CA's PEM bundle.

### 3.9 First-startup bootstrapping

On first startup with an empty database, the server executes (in a single
atomic transaction):

1. Creates builtin roles: `admin`, `builder`, `viewer` (with predefined
   capabilities).
2. Creates a user record for `seed_admin` (from server config).
3. Assigns the `admin` role to the seed admin user.
4. For each entry in `seed_worker_api_keys`: creates an API key owned by
   the seed admin, stores the argon2 hash.
5. Commits the transaction.
6. Prints plaintext API keys to stdout (only after commit succeeds).

**Operator action:** Copy the printed API key(s) into the worker config(s)
(`worker.yaml` → `api_key` field). This is a one-time step.

Additional worker API keys can be created later via the REST API:
`POST /api/auth/api-keys` (requires `apikeys:create:own` or
`permissions:manage`).

---

## 4. Migration from Python cbsd

### 4.1 Overview of operator actions at cutover

1. Stop the Python `cbsd` server and worker.
2. Stop Redis.
3. Run the permissions migration script (YAML to SQLite).
4. Start the Rust `cbsd-rs` server (generates new database, seeds
   roles/admin).
5. Start the Rust `cbsd-rs` worker (connects via WebSocket).
6. Notify users to update `cbc` and re-authenticate (`cbc login`).

### 4.2 Release ordering

The cutover requires a coordinated release of three packages:

1. **`cbsdcore`** -- updated Pydantic models with new build states
   (`dispatched`, `revoking`, `revoked`), lowercase state names, `aarch64`
   as canonical arch value. This must be published first because both old
   `cbc` and new `cbc` depend on it.

2. **`cbc`** -- updated API paths, SSE log streaming, new build states,
   field renames (`task_id` dropped, `submitted` to `submitted_at`,
   `desc` to `descriptor`, `user` to `user_email`). Must be released
   before or alongside the Rust server deployment.

3. **`cbsd-rs` server + worker** -- deployed after cbsdcore and cbc are
   available.

4. **User notification** -- users update `cbc` to the new version and run
   `cbc login` to get a fresh PASETO token.

### 4.3 Token migration

**Approach: accept the break.** The Rust server starts with an empty `tokens`
table. Python-era PASETO tokens use ISO 8601 timestamps; Rust uses epoch
integers. SHA-256 hashes are incompatible. All existing tokens will be
rejected with 401.

Users re-authenticate once with `cbc login`. Tokens default to infinite TTL,
so this is a one-time cost.

### 4.4 Permissions migration

The current `permissions.yaml` (generated by `do-cbs-compose.sh`, see
`/mnt/pci5-dev/clyso/cbs.git/do-cbs-compose.sh` lines 200--263) needs to be
converted to SQLite records.

**Migration script** (one-time, run after first server startup):

```
cbsd-rs/scripts/migrate-permissions.py
```

This script:

1. Reads the existing `permissions.yaml`.
2. For each YAML `group`: creates a role in SQLite with the corresponding
   capabilities.
3. Converts `authorized_for` entries:
   - `type: project` becomes scope type `channel` (per design decision).
   - `pattern` regex patterns are converted to glob patterns.
   - `caps` are mapped to the new capability strings.
4. For each YAML `rule`: creates user-role assignments matching
   `user_pattern` (regex) against known users.
5. Writes results to the SQLite database.

**Note:** The `seed_admin` from the server config will already have the
`admin` role from bootstrapping. The migration script should skip
duplicate assignments.

### 4.5 Build ID continuity

The Rust server uses `AUTOINCREMENT` for `builds.id`. On a fresh database,
this starts at 1. If the Python system had existing builds, the initial
migration must set the autoincrement counter to `MAX(existing_id) + 1` to
avoid collisions with retained log files on disk.

If no build history is being preserved, this is not needed.

### 4.6 Data directories

| Python path | Rust equivalent | Migration action |
|---|---|---|
| `_local/cbs/data/db/` (dbm) | `_local/cbsd-rs/data/cbsd.db` (SQLite) | Run migration script or start fresh |
| `_local/cbs/logs/builds/` | `_local/cbsd-rs/logs/builds/` | Copy or symlink (log format unchanged) |
| `_local/cbs/redis/` | (not needed) | Archive and remove |
| `_local/cbs/config/server/` | `_local/cbsd-rs/config/server/` | Write new `server.yaml` |
| `_local/cbs/config/worker/` | `_local/cbsd-rs/config/worker/` | Write new `worker.yaml` + keep cbscore config |
| `_local/cbs/scratch/` | `_local/cbs/scratch/` (unchanged) | No change needed |

---

## 5. Codebase Changes Required

### 5.1 New files

| Path | Description |
|---|---|
| `container/ContainerFile.cbsd-rs` | Multi-stage Dockerfile: Rust build, server image, worker image |
| `container/entrypoint-cbsd-rs-server.sh` | Server container entrypoint |
| `container/build-rs.sh` | Build script for Rust container images |
| `podman-compose.cbsd-rs.yaml` | Compose file for dev/staging (server + worker, no Redis) |
| `cbsd-rs/scripts/cbscore-wrapper.py` | Python bridge: receives JSON on stdin, invokes cbscore, emits structured result |
| `cbsd-rs/scripts/migrate-permissions.py` | One-time migration: `permissions.yaml` to SQLite |
| `systemd/templates/systemd/cbsd-rs-server@.service` | Systemd unit for containerized server |
| `systemd/templates/systemd/cbsd-rs-worker@.service` | Systemd unit for containerized worker |

### 5.2 Modified files

| Path | Change |
|---|---|
| `container/build.sh` | No change (continues to build Python images) |
| `do-cbs-compose.sh` | Add `--rs` flag or new command to support `podman-compose.cbsd-rs.yaml` |
| `.github/workflows/release-container-images.yaml` | Add job for building Rust container images alongside Python ones |

### 5.3 Files that remain unchanged

| Path | Reason |
|---|---|
| `container/ContainerFile.cbsd` | Python images continue to be produced until Python cbsd is fully deprecated |
| `container/entrypoint.sh` | Python server entrypoint, unchanged |
| `container/release.sh` | Tag-based release, unchanged (Rust images use same tag scheme) |
| `systemd/templates/systemd/cbsd-network@.service` | Still needed for Python deployments; not needed for Rust |
| `systemd/templates/systemd/cbsd-.service.in` | Python container service template, unchanged |

### 5.4 CI/CD changes

The existing `.github/workflows/release-container-images.yaml` builds Python
images using `container/build.sh`. For the Rust port, add a parallel job:

```yaml
  build-and-push-rust:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write
    env:
      REGISTRY: ghcr.io
      IMAGE_PREFIX: ${{ github.repository }}
    steps:
      - uses: actions/checkout@v6
        with:
          ref: ${{ inputs.tag_name }}

      - name: Log in to GHCR
        run: |
          echo "${{ secrets.GITHUB_TOKEN }}" | \
            podman login ${{ env.REGISTRY }} -u ${{ github.actor }} --password-stdin

      - name: Build and push cbsd-rs images
        run: |
          TAG="${{ inputs.tag_name || github.ref_name }}"

          podman build -f container/ContainerFile.cbsd-rs \
            --target cbsd-rs-server \
            -t ${{ env.REGISTRY }}/${{ env.IMAGE_PREFIX }}/cbsd-rs-server:${TAG} .

          podman build -f container/ContainerFile.cbsd-rs \
            --target cbsd-rs-worker \
            -t ${{ env.REGISTRY }}/${{ env.IMAGE_PREFIX }}/cbsd-rs-worker:${TAG} .

          podman push ${{ env.REGISTRY }}/${{ env.IMAGE_PREFIX }}/cbsd-rs-server:${TAG}
          podman push ${{ env.REGISTRY }}/${{ env.IMAGE_PREFIX }}/cbsd-rs-worker:${TAG}
```

### 5.5 The cbscore-wrapper.py bridge script

Located at `cbsd-rs/scripts/cbscore-wrapper.py`. This is the Python process
that the Rust worker spawns as a subprocess. It:

1. Reads a JSON object from stdin: `{descriptor, component_path, trace_id}`.
2. Sets `CBS_TRACE_ID` environment variable for cbscore logging.
3. Writes `cbscore.config.yaml` overrides (component path from the unpacked
   tarball).
4. Invokes `cbscore.runner.runner()` with the build descriptor.
5. Streams all stdout/stderr output (the Rust worker captures this).
6. On completion, emits a structured JSON result line:
   `{"type": "result", "exit_code": 0, "error": null}`
7. Exits with a classified exit code (0=success, 1=failure, 2=infra error).

The worker recognizes the result line by prefix matching
(`line.starts_with('{"type":"result"')`) and extracts the `error` field.

---

## 6. Operational Notes

### 6.1 Graceful server restart (rolling deploy)

Send `SIGTERM` to the server. Behavior:

1. Stops accepting new HTTP connections.
2. Does NOT revoke running builds.
3. Closes WebSocket connections.
4. Workers detect the drop, enter reconnect loop with exponential backoff.
5. New server instance starts, workers reconnect, in-flight builds resume.

### 6.2 Intentional server decommission

Send `SIGQUIT` or start with `--drain`:

1. Stops accepting new connections.
2. Sends `build_revoke` to all workers with active builds.
3. Waits up to 30s drain timeout for `build_finished` acknowledgments.
4. Unacknowledged builds marked `failure`.
5. Shuts down.

### 6.3 Worker scaling

To add a worker:

1. Create an API key via `POST /api/auth/api-keys`.
2. Configure a new worker with the key and the server URL.
3. Start the worker -- it connects and registers automatically.

No server restart required. No Redis or Celery configuration needed.

### 6.4 Component updates

Components live on the server filesystem (`components/` directory). To update:

1. Replace files in the `components/` directory.
2. No restart needed -- the server re-reads components on each build dispatch.

Workers do not need to be restarted or updated for component changes.

### 6.5 Health checks

| Check | Python stack | Rust stack |
|---|---|---|
| Server alive | HTTP GET to `:8080` | HTTP GET to `:8080` (same) |
| Worker alive | `celery inspect ping` | `GET /api/workers` (shows connected workers) |
| Redis alive | `redis-cli ping` | N/A (no Redis) |

### 6.6 Monitoring

The server's `GET /api/admin/queue` endpoint (requires `admin:queue:view`)
returns the full queue state including active builds, worker assignments, and
queue depths per priority lane. This replaces Celery's `flower` or `celery
events` monitoring.
