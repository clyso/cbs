# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Project Overview

CES Build System (CBS) — a collection of tools by Clyso to automate
building and releasing containers for Ceph and other components.

### Python workspace (UV, Python 3.13+)

Three main tools:

- **cbsbuild** (`cbscore` package) — CLI for building containers (GPLv3)
- **crt** — CLI for managing Ceph releases (GPLv3)
- **cbs** (`cbsd` package) — REST API server + Celery worker queue for
  distributed builds (AGPLv3)

Supporting packages:

- **cbsdcore** — shared Pydantic models/types used by both `cbsd` and `cbc`
  (AGPLv3)
- **cbc** — CLI client for the CBS service (GPLv3)

### Rust workspace (`cbsd-rs/`)

Rust reimplementation of the CBS build service daemon. Replaces the Python +
FastAPI + Celery + Redis stack with Rust + axum + SQLite + WebSocket.

- **`cbsd-proto`** — shared wire types (BuildDescriptor, BuildState, WebSocket
  messages); no IO dependencies
- **`cbsd-server`** — REST API (axum), Google OAuth, PASETO tokens, RBAC,
  build queue, WebSocket handler, SSE log streaming, SQLite persistence (sqlx)
- **`cbsd-worker`** — WebSocket client, build executor (spawns cbscore Python
  subprocess), SIGTERM/SIGKILL process management
- **`cbc`** — Rust CLI client for the CBS service

See `cbsd-rs/CLAUDE.md` for detailed guidance on the Rust workspace.

## Build & Development Commands

### Python workspace

```bash
# Install all dependencies (UV workspace)
uv sync

# Run cbsbuild CLI
uv run cbsbuild

# Run cbc client CLI
uv run cbc

# Run crt CLI
uv run crt

# Run the CBS server (FastAPI + Uvicorn)
uv run python cbsd/cbs-server.py

# Local dev environment (server + worker + Redis)
./do-cbs-compose.sh        # uses podman-compose.cbs.yaml
```

### Rust workspace (`cbsd-rs/`)

```bash
cd cbsd-rs

cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo fmt --all --check

# After any migration or query change:
DATABASE_URL=sqlite:///tmp/cbsd-dev.db cargo sqlx prepare --workspace

# Local dev environment (server + worker)
# Run from repo root:
podman-compose -f podman-compose.cbsd-rs.yaml up
```

## Linting, Formatting & Type Checking

### Python workspace

```bash
# Lint (ruff)
uv run ruff check

# Format check (ruff)
uv run ruff format --check

# Format fix
uv run ruff format

# Type check (basedpyright, strict "all" mode)
uv run basedpyright .
```

### Rust workspace (`cbsd-rs/`)

Run these in order before every commit:

```bash
cargo fmt --all           # 1. format
cargo clippy --workspace  # 2. lint (fix all warnings)
cargo check --workspace   # 3. compile check (SQLX_OFFLINE=true if no DB)
```

Pre-commit hooks are configured via Lefthook (`.lefthook.yaml`):

- Python: `ruff check`, `ruff format --check`, `basedpyright .`
- Markdown: `markdownlint-cli2`, `prettier` (via yarn)
- Shell: `shfmt` (via Go)

## Workspace Structure

### Python workspace

```
pyproject.toml          # Root workspace config, ruff rules, dev deps
cbscore/                # Core build library (src layout: cbscore/src/cbscore/)
  builder/              # Container/RPM build logic
  containers/           # Container image handling (podman/buildah)
  releases/             # Release management
  versions/             # Version descriptor handling
  cmds/                 # CLI commands (Click)
cbsd/                   # Build service daemon
  cbs-server.py         # FastAPI entry point
  cbslib/               # Server library
    auth/               # OAuth, user auth
    builds/             # Build tracking, SQLite DB
    routes/             # FastAPI route handlers
    worker/             # Celery worker tasks
    core/               # Core manager & monitor
cbsdcore/               # Shared API types (src layout: cbsdcore/src/cbsdcore/)
  api/                  # Request/response models (Pydantic)
  auth/                 # Auth models
  builds/               # Build state types
cbc/                    # CLI client for CBS (src layout: cbc/src/cbc/)
  client.py             # HTTP client
  cmds/                 # CLI commands
crt/                    # Ceph Release Tool (src layout: crt/src/crt/)
components/             # Component definitions (e.g., Ceph build descriptors)
container/              # Dockerfiles and scripts for CBS container images
```

### Rust workspace

```
cbsd-rs/
  Cargo.toml            # Workspace root
  Cargo.lock
  .sqlx/                # sqlx offline query cache (committed)
  migrations/           # sqlx SQL migrations (embedded by server)
  scripts/              # cbscore-wrapper.py (Python subprocess bridge)
  cbsd-proto/           # Shared types crate (wire format, no IO)
  cbsd-server/          # Server binary (axum REST + WebSocket + SSE)
  cbsd-worker/          # Worker binary (WS client + subprocess executor)
  cbc/                  # CLI client for CBS service (Rust)
  docs/                 # Design docs, implementation plans, reviews
    cbsd-rs/design/     # Authoritative architecture & design documents
    cbsd-rs/plans/      # Phased implementation plans with progress tracking
    cbc/design/         # cbc CLI design documents
```

## Key Technologies

### Python workspace

- **CLI**: Click 8.1
- **Web**: FastAPI + Uvicorn
- **Task queue**: Celery 5.6 + Redis
- **Data validation**: Pydantic v2
- **Auth**: Authlib (OAuth), pyseto (PASETO tokens)
- **Secrets**: HashiCorp Vault (hvac)
- **Storage**: aioboto3 (S3), SQLite (build tracking)
- **Containers**: Podman/Buildah (not Docker)

### Rust workspace (`cbsd-rs/`)

- **Web**: axum (HTTP + WebSocket + SSE)
- **Async runtime**: Tokio
- **Auth**: Google OAuth, PASETO tokens (pasetors), tower-sessions (BFF
  session cookies)
- **Database**: SQLite via sqlx (async, compile-time checked queries)
- **Serialization**: serde / serde_json
- **CLI** (`cbc`): clap

## Code Conventions

- All packages use `src` layout (e.g., `cbscore/src/cbscore/`)
- Pydantic models for all data structures and API types
- Async patterns throughout (aiofiles, aiohttp, AsyncGenerator)
- Custom exception hierarchy (e.g., `CESError`, `ConfigError`)
- Click decorators for CLI commands with `Ctx` context objects
- Ruff rules: extensive set including security (S/bandit), complexity (C4, SIM),
  naming (N), imports (I/isort). See `pyproject.toml` `[tool.ruff.lint]` for
  full config
- basedpyright with `typeCheckingMode: "all"` (strictest)

## Contributing

- DCO (Developer Certificate of Origin) required on all commits
- GPG-signed commits required
- Commit style follows Ceph project conventions

## Claude Code Workflow

When making code changes, Claude should:

1. Complete a single, coherent, testable change
2. Use the `/git-commit-messages` skill to determine if the change is ready for
   commit and to propose an appropriate commit message
3. Request review of the commit message before proceeding
4. Only proceed with additional changes after getting approval

This ensures commits are logical, atomic, and well-documented.
