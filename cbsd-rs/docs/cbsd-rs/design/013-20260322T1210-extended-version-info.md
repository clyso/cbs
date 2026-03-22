# 013 — Extended Version Info

## Status

Draft v2 — addresses review v1 + user feedback

## Problem

The server, worker, and CLI binaries report only their
`Cargo.toml` version (e.g., `0.1.0`). There is no way to
determine which exact commit a running binary was built
from. In a development workflow where binaries are rebuilt
frequently without version bumps, two binaries with the
same `0.1.0` version can behave differently because they
were compiled from different commits. The server's health
endpoint returns `{"status": "ok"}` with no version
information, so operators cannot verify what code is
running without SSH access.

Additionally, when the server communicates with workers
over WebSocket, there is no version exchange — the server
cannot detect version skew between itself and connected
workers.

## Design

### Version Format

All binaries use a semver build-metadata string:

```
<cargo-version>+g<abbrev-sha>
```

Examples:

- `0.1.0+g3a7f2b1` — production build from commit
  `3a7f2b1`
- `0.1.0+unknown` — development build (no git info
  available)

The `g` prefix follows the `git describe` convention.
Development builds (host or dev container) always show
`unknown` — we do not attempt to resolve git info outside
of production container builds. This is intentional:
development binaries are rebuilt constantly and the commit
info is not operationally meaningful.

### Two Build Contexts

| Context | Git info source | Version example |
|---------|-----------------|-----------------|
| Dev (host `cargo build`, cargo-watch in container) | None — always `unknown` | `0.1.0+unknown` |
| Prod (container image via `container/build.sh`) | `--build-arg` from host `git describe` → `.git-version` file in builder stage | `0.1.0+g3a7f2b1` |

### Compile-Time Embedding via `build.rs`

A `build.rs` script in each binary crate (`cbsd-server`,
`cbsd-worker`, `cbc`) reads a `.git-version` file from
the workspace root at compile time. If the file does not
exist or is empty, the version defaults to `unknown`.

```rust
// build.rs
fn main() {
    // Look for .git-version in the workspace root.
    let workspace_root = std::path::Path::new(
        env!("CARGO_MANIFEST_DIR"),
    )
    .parent()
    .expect("crate must be in a workspace");

    let version_file = workspace_root.join(".git-version");

    let git_meta = std::fs::read_to_string(&version_file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|sha| format!("g{sha}"))
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=CBS_BUILD_META={git_meta}");

    // Re-run if the version file appears or changes.
    println!(
        "cargo:rerun-if-changed={}",
        version_file.display()
    );
}
```

Each binary constructs its extended version string:

```rust
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "+",
    env!("CBS_BUILD_META"),
);
```

### `.git-version` File

This file exists **only inside the production builder
container**, created by a `RUN` step after the source is
copied. It is never committed to the repo, never written
to the host, and never present in dev builds.

### Production Container Build

A new script `container/build-cbsd-rs.sh` (modelled on
the existing `container/build.sh` for cbsd) handles
production image builds. It:

1. Runs `git describe --always --match=''` on the host
   to get the abbreviated SHA
2. Passes it as `--build-arg GIT_VERSION=<sha>` to
   `podman build`
3. The Containerfile's `rust-builder` stage writes the
   file before `cargo build`:

```dockerfile
FROM alpine:3.21 AS rust-builder

ARG GIT_VERSION=unknown
# ...
COPY cbsd-rs/ .
RUN echo "${GIT_VERSION}" > .git-version
RUN cargo build --release --workspace
```

The `.git-version` file exists ephemerally inside the
builder stage only. It is not copied to the final runtime
images — only the compiled binary (which has the version
baked in) is copied.

### Podman-Compose: Development Only

The `podman-compose.cbsd-rs.yaml` file drops its
production profiles (`server`, `worker`). It becomes
development-only. Production deployments use pre-built
images from `container/build-cbsd-rs.sh` or a CI
pipeline.

This is documented in the cbsd-rs README.

### Shared `build.rs` Logic

Since all three binary crates need the same `build.rs`,
the logic is duplicated (~15 lines each). This avoids
adding a build-dependency crate to the workspace. If it
grows, extract later.

### Server: `/api/health` Endpoint

The existing health endpoint changes from:

```json
{"status": "ok"}
```

to:

```json
{"status": "ok", "version": "0.1.0+g3a7f2b1"}
```

This endpoint remains unauthenticated. Monitoring tools
and operators can poll it to verify which code is
deployed.

### CLI: `cbc --version`

clap's `#[command(version)]` attribute is overridden
with the extended version:

```rust
#[derive(Parser)]
#[command(
    name = "cbc",
    version = VERSION,
)]
struct Cli { ... }
```

Output:

```
$ cbc --version
cbc 0.1.0+g3a7f2b1
```

In development: `cbc 0.1.0+unknown`.

### `cbsd-server --version` / `cbsd-worker --version`

Same pattern as `cbc`.

### Worker: Version in `Hello` Message

The worker's `Hello` WebSocket message gains a `version`
field:

```json
{
  "type": "Hello",
  "protocol_version": 2,
  "arch": "x86_64",
  "cores_total": 8,
  "ram_total_mb": 32768,
  "version": "0.1.0+g3a7f2b1"
}
```

The server logs the worker version at connection time.
When the worker version differs from the server version,
the server logs at WARN level to surface version skew.
No enforcement — just visibility.

The field is optional (`#[serde(default)]`) for backwards
compatibility with older workers that don't send it.

The server stores the reported version in its in-memory
worker state (`WorkerState::Connected` gains a
`version: Option<String>` field). The
`GET /api/workers` endpoint includes `version` in each
worker's record. Offline workers show `version: null`.

## Files Changed

| File | Change |
|------|--------|
| `cbsd-server/build.rs` | New: read `.git-version` |
| `cbsd-worker/build.rs` | New: read `.git-version` |
| `cbc/build.rs` | New: read `.git-version` |
| `cbsd-server/src/main.rs` | `VERSION` const, `--version` |
| `cbsd-server/src/app.rs` | Health endpoint returns version |
| `cbsd-worker/src/main.rs` | `VERSION` const, `--version` |
| `cbc/src/main.rs` | `VERSION` const, `--version` |
| `cbsd-proto/src/ws.rs` | `Hello` gains `version` field |
| `cbsd-worker/src/ws/handler.rs` | Include version in `Hello` |
| `cbsd-server/src/ws/handler.rs` | Log worker version, warn on skew |
| `cbsd-server/src/ws/liveness.rs` | `WorkerState::Connected` gains `version` |
| `cbsd-server/src/routes/workers.rs` | `WorkerInfoResponse` gains `version` |
| `container/ContainerFile.cbsd-rs` | `ARG GIT_VERSION`, `RUN echo` in builder |
| `container/build-cbsd-rs.sh` | New: production build script |
| `podman-compose.cbsd-rs.yaml` | Remove prod profiles |
| `cbsd-rs/README.md` | Document prod vs dev deployment |
| `cbc/src/worker.rs` | Display version in worker list |
