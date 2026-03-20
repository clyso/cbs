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
