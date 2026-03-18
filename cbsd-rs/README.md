# cbsd-rs — CBS Build Service Daemon (Rust)

Rust rewrite of `cbsd/`. Replaces the FastAPI + Celery + Redis stack with
axum + SQLite + WebSocket. Two binaries:

- **cbsd-server** — REST API, build queue, WebSocket endpoint for workers
- **cbsd-worker** — WebSocket client, cbscore build subprocess bridge

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
setup instead.

Two compose profiles are available:

| Profile | Command flag | Description |
|---------|-------------|-------------|
| `dev`   | `--dev`     | cargo-watch auto-reload; source bind-mounted at `/cbs/src` |
| `prod`  | _(default)_ | pre-built local binaries; suitable for staging |

The `do-cbsd-rs-compose.sh` helper script automates config generation and
image management. It requires two external files — a Google OAuth2 client
secrets JSON and a cbscore `cbs-build.config.yaml` — that are not committed
to the repository.

**Dev quickstart** (pre-configured worker API key, no REST API call needed):

```bash
./do-cbsd-rs-compose.sh prepare --dev \
    --google-client-secrets ~/client_secret.json \
    --cbscore-config ~/cbs-build.config.yaml \
    --seed-admin you@example.com
./do-cbsd-rs-compose.sh up --dev
```

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
