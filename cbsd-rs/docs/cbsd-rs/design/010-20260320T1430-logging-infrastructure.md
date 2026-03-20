# 010 — Logging Infrastructure

## Status

Draft v3 — logrotate for rotation/compression

## Problem

Two issues exist in the current logging implementation:

### 1. Build log path mismatch (bug)

The dispatch code stores `builds/{id}/build.log` in the
`build_logs` DB table, but the writer creates `builds/{id}.log`
(flat file). Two consumers read the path from the DB and will
therefore look in the wrong place:

| Code path | Path used | Source | Broken? |
|-----------|-----------|--------|---------|
| `ws/dispatch.rs:114` | `builds/{id}/build.log` | hardcoded → DB | origin |
| `logs/writer.rs:76` | `builds/{id}.log` | hardcoded | correct |
| `routes/builds.rs:421` (tail) | `builds/{id}.log` | hardcoded | correct |
| `routes/builds.rs:482` (full) | `builds/{id}.log` | hardcoded | correct |
| `logs/sse.rs:68` (follow) | DB `log_path` | from DB | **broken** |
| `logs/gc.rs:101` (GC) | DB `log_path` | from DB | **broken** |

### 2. Application logging to file not wired (gap)

Both `cbsd-server` and `cbsd-worker` log exclusively to stdout
via `tracing_subscriber::fmt()`. The server config has a
`log_file: Option<PathBuf>` field in `LoggingConfig` but it is
marked `#[allow(dead_code)]` and never read. The worker has no
logging config at all.

The Python cbsd supports dual-target logging (file + console)
with a `RotatingFileHandler` (10 MB, 1 backup). The Rust port
must provide file-based logging for production and restrict
console output to development mode only.

## Design

### Fix 1: Build log path — one-line change

Change `ws/dispatch.rs:114` from:

```rust
let log_path = format!(
    "builds/{}/build.log", build.build_id.0
);
```

to:

```rust
let log_path = format!("builds/{}.log", build.build_id.0);
```

This aligns the DB entry with the writer, tail, and
full-download routes. The SSE handler and GC — which correctly
read from the DB — will then resolve to the actual file.

No migration needed: the `build_logs` table is transient (rows
are created at dispatch time and GC'd after retention). Any stale
rows from a pre-fix server instance will simply not find a file
at the old path, and GC will clean up the row on its next cycle
(the `NotFound` case is already handled gracefully in
`gc.rs:110-115`).

### Fix 2: Structured logging with file output

#### Output mode rules

| Mode | Console (stdout) | File | Startup validation |
|------|-------------------|------|--------------------|
| Dev (`CBSD_DEV=1`) | always | if `log-file` set | none — console is always available |
| Prod (no `CBSD_DEV`) | never | **required** | hard error if `log-file` missing |

In development mode, console output is always enabled. File
output is additionally enabled when `logging.log-file` is
configured. This means dev mode logs to both targets when a
`log-file` path is present.

In production mode (no `CBSD_DEV`), console output is never
produced. File output is the only target — if `log-file` is not
configured, the binary refuses to start with a clear error.

#### Dev mode detection

Both binaries check the `CBSD_DEV` environment variable. Any
non-empty value enables development mode (console output).

The server additionally reads `config.dev.enabled` for other
dev-mode behaviors (OAuth bypass, worker seeding). For logging
specifically, `CBSD_DEV` is the single source of truth in both
binaries.

#### Subscriber topology

Both binaries build a `tracing_subscriber` layer stack at
startup using the `tracing_subscriber::registry()` + `.with()`
combinator pattern:

```
              ┌──────────────┐
              │  EnvFilter   │  ← RUST_LOG or config level
              └──────┬───────┘
                     │
        ┌────────────┴────────────┐
        │                         │
┌───────▼───────┐       ┌────────▼────────┐
│  fmt (stdout)  │       │  fmt (file)     │
│  CBSD_DEV only │       │  always when    │
│                │       │  log-file set   │
└────────────────┘       └─────────────────┘
```

#### File output

`tracing-appender` is already a dependency of `cbsd-server`
(version 0.2.4, currently unused). It will be added to
`cbsd-worker` as well. This is not a new dependency.

The file layer uses `tracing_appender::rolling::never()` —
the application writes to a single, stable file (e.g.,
`/cbs/logs/server.log`). **The application does not rotate
files.** All rotation, compression, and retention is handled
by `logrotate` on the host (see below).

Using `Rotation::NEVER` means:

- The configured `log-file` path is the exact file written to
- No date suffixes, no prefix/suffix splitting
- The file grows until logrotate rotates it via `copytruncate`

**Guard lifetime** — `tracing_appender::non_blocking()` returns
a `WorkerGuard` that must be held for the process lifetime
(dropping it flushes and stops the writer thread). Both `main()`
functions store the guard in a `let _guard = ...;` binding.

#### `log-file` path validation

The `log-file` config value is the exact file path written to.

Validation rules (checked at startup, before tracing init):

1. **Must be absolute.** Relative paths resolve against the
   container CWD, which may be `/` or an unexpected location.
   Reject with a clear error.
2. **Must have a non-empty filename.** Trailing slashes or
   bare directories (e.g., `/cbs/logs/`) are rejected.
3. **Parent directory must exist** (or be creatable).

#### Startup validation

If `CBSD_DEV` is not set and no `log-file` is configured, the
binary **refuses to start** with a panic message:

```
config error: logging.log-file is required when CBSD_DEV is
not set — in production mode there is no console output, so
a log file path must be configured
```

**Server:** validated in `ServerConfig::validate()`.
**Worker:** validated in `WorkerConfig::resolve()`.

#### Config changes

**Server** (`config.rs` / `server.yaml.example`):

No schema change — `LoggingConfig` already has `level` and
`log_file`. Remove `#[allow(dead_code)]` from `log_file` and
wire it up. Add validation for prod-mode-requires-log-file.

**Worker** (`config.rs` / `worker.yaml.example`):

Add a `logging` section:

```yaml
logging:
  level: info
  log-file: /cbs/logs/worker.log
```

Deserialized as:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    pub log_file: Option<PathBuf>,
}
```

#### Compose file changes

`podman-compose.cbsd-rs.yaml`:

- **prod server/worker**: generated configs include
  `logging.log-file`. No `CBSD_DEV` set — console disabled.
- **dev server-dev/worker-dev**: add `CBSD_DEV: "1"` to
  environment section. `log-file` can optionally be configured
  for dual-target dev logging.

#### Log rotation and compression (logrotate)

The application writes to a single file and never rotates it.
All rotation, compression, and retention is handled by the
standard `logrotate` utility on the host.

**Why logrotate, not tracing-appender rotation:**

- `tracing-appender` supports daily rotation but **cannot
  compress** old files. It can only delete them.
- logrotate is the standard Linux log management tool. It
  handles rotation, compression, and retention in one config.
- Using `Rotation::NEVER` in the application + logrotate on
  the host gives a clean separation: the application writes,
  the OS manages.

**logrotate config** — generated per deployment by
`install.sh`:

```
# /path/to/deployment/logrotate.conf
{{cbsd_rs_data}}/{{deployment}}/*/logs/*.log {
    daily
    rotate 30
    compress
    delaycompress
    missingok
    notifempty
    copytruncate
}
```

Key directives:

| Directive | Effect |
|-----------|--------|
| `daily` | Rotate once per day |
| `rotate 30` | Keep 30 rotated files (30 days) |
| `compress` | gzip rotated files |
| `delaycompress` | Keep yesterday's file uncompressed (allows tailing) |
| `copytruncate` | Copy the file, then truncate the original in place — the application keeps writing to the same FD without interruption |
| `missingok` | No error if a log file is missing |
| `notifempty` | Skip rotation if the file is empty |

The glob `*/logs/*.log` covers both `server/logs/server.log`
and `worker*/logs/worker.log` subdirectories.

**`copytruncate` and `non_blocking`:**

`tracing_appender::non_blocking` wraps the file writer in a
background thread that flushes buffered writes. When logrotate
runs `copytruncate`, it copies the file contents then
truncates the original to zero. The application's open FD
remains valid — subsequent writes go to the now-empty file.
There is a small window (microseconds) where lines written
between the copy and truncate may appear in both the old and
new file. This is the standard trade-off for `copytruncate`
and is acceptable for application logs.

**File lifecycle on disk:**

```
Day 0:  server.log                  (active)
Day 1:  server.log                  (active, truncated)
        server.log.1                (yesterday, uncompressed)
Day 2:  server.log                  (active, truncated)
        server.log.1                (yesterday, uncompressed)
        server.log.2.gz             (2 days ago, compressed)
Day 31: server.log.30.gz            (oldest, deleted next)
```

**systemd timer** — runs logrotate daily for each
deployment:

Timer template (`cbsd-rs-logrotate@.timer`):

```ini
[Unit]
Description=Rotate cbsd-rs logs for '%i' deployment

[Timer]
OnCalendar=*-*-* 00:00:00
Persistent=true
RandomizedDelaySec=300

[Install]
WantedBy=timers.target
```

Service template (`cbsd-rs-logrotate@.service`):

```ini
[Unit]
Description=Rotate cbsd-rs logs for '%i' deployment

[Service]
Type=oneshot
ExecStart=/usr/sbin/logrotate \
    --state {{cbsd_rs_data}}/logrotate.%i.state \
    {{cbsd_rs_data}}/%i/logrotate.conf
```

The `--state` flag stores logrotate's rotation state (which
files were rotated when) in a per-deployment state file.
This avoids conflicting with the system-wide logrotate state
at `/var/lib/logrotate/logrotate.status`.

`Persistent=true` ensures missed runs are caught up on boot.
`RandomizedDelaySec=300` avoids simultaneous runs across
deployments.

**Install changes:**

`install.sh` generates `logrotate.conf` at
`${data_dir}/${deployment}/logrotate.conf` with the
`{{cbsd_rs_data}}` and `{{deployment}}` placeholders
resolved. It installs and enables the timer once per
deployment, alongside the existing target and service
templates. The timer is enabled unconditionally — logrotate
is a no-op when no log files exist (`missingok`).

**Compose deployments:**

The compose deployment does not use logrotate. For compose:

- Dev deployments are short-lived and use console logging
  primarily (`CBSD_DEV=1`). Log files grow unbounded if
  configured, but this is acceptable for development.
- Prod-like compose (staging) can rely on the container
  runtime's log management or an operator-provisioned
  logrotate. Documenting this in the README is sufficient.

#### Systemd deployment changes

The `do-cbsd-rs-compose.sh prepare` script already generates
server and worker YAML configs. For production (non-`--dev`)
mode, the generated configs must include `logging.log-file`
pointing to `/cbs/logs/server.log` and `/cbs/logs/worker.log`
respectively. The systemd `cbsd-rs-ctr.sh` already creates and
mounts these directories.

For `--dev` mode, `CBSD_DEV=1` should be passed as an additional
`-e` flag in the `podman run` commands within `cbsd-rs-ctr.sh`,
but only when the server's `dev.enabled` is true. In practice,
the systemd deployment is production-only and does not use dev
mode, so this is a no-op for the current deployment.

#### README update

Document `CBSD_DEV` in the Development section of
`cbsd-rs/README.md`, explaining:

- `CBSD_DEV=1` enables console output (both server and worker)
- Without it, only file logging is active
- The compose dev profile sets this automatically
- **Distinction from `dev.enabled`**: `CBSD_DEV` controls
  console logging only. The server config `dev.enabled`
  controls OAuth bypass and worker seeding. These are
  independent: setting `dev.enabled: true` without
  `CBSD_DEV=1` gives OAuth bypass but file-only logging.
  The compose `--dev` flag sets both.

### Shared logging setup code

To avoid duplicating the subscriber construction in two
binaries, a shared function could live in `cbsd-proto`. However,
`cbsd-proto` is currently zero-IO (no async, no tracing
dependency). Pulling in `tracing-subscriber` and
`tracing-appender` would break that design constraint.

**Decision:** duplicate the ~25 lines of subscriber setup in
each binary's `main.rs`. This is small, self-contained, and
avoids coupling the shared types crate to tracing
infrastructure.

## Implementation sketch

```rust
use std::path::Path;

use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

fn is_dev_mode() -> bool {
    std::env::var("CBSD_DEV")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Validate a log-file path: must be absolute and have
/// a filename component. Returns (directory, filename).
fn validate_log_path(path: &Path) -> (&Path, &str) {
    assert!(
        path.is_absolute(),
        "config error: logging.log-file must be an \
         absolute path, got '{}'",
        path.display()
    );
    let dir = path.parent().unwrap_or_else(|| {
        panic!(
            "config error: logging.log-file has no \
             parent directory: '{}'",
            path.display()
        )
    });
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| {
            panic!(
                "config error: logging.log-file has \
                 no filename component: '{}'",
                path.display()
            )
        });
    (dir, filename)
}

fn setup_tracing(
    level: &str,
    log_file: Option<&Path>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let is_dev = is_dev_mode();

    // Prod mode requires a log file.
    if !is_dev && log_file.is_none() {
        panic!(
            "config error: logging.log-file is required \
             when CBSD_DEV is not set — in production \
             mode there is no console output, so a log \
             file path must be configured"
        );
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    let console_layer = if is_dev {
        Some(fmt::layer().with_ansi(true))
    } else {
        None
    };

    let (file_layer, guard) =
        if let Some(path) = log_file {
            let (dir, filename) =
                validate_log_path(path);
            // Rotation::NEVER — single stable file.
            // logrotate on the host handles rotation,
            // compression, and retention.
            let appender =
                tracing_appender::rolling::never(
                    dir, filename,
                );
            let (writer, guard) =
                tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            (Some(layer), Some(guard))
        } else {
            (None, None)
        };

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    guard
}
```

## Files changed

| File | Change |
|------|--------|
| `cbsd-server/src/ws/dispatch.rs` | Fix log path format |
| `cbsd-server/src/config.rs` | Remove dead_code, add validation |
| `cbsd-server/src/main.rs` | Build layered subscriber |
| `cbsd-worker/Cargo.toml` | Add `tracing-appender` dep |
| `cbsd-worker/src/config.rs` | Add `LoggingConfig` struct |
| `cbsd-worker/src/main.rs` | Build layered subscriber |
| `config/server.yaml.example` | Document behavior |
| `config/worker.yaml.example` | Add `logging` section |
| `podman-compose.cbsd-rs.yaml` | Add `CBSD_DEV` to dev |
| `cbsd-rs/README.md` | Document `CBSD_DEV` env var |
| `systemd/templates/systemd/cbsd-rs-logrotate@.timer` | New: daily timer |
| `systemd/templates/systemd/cbsd-rs-logrotate@.service` | New: runs logrotate |
| `systemd/install.sh` | Generate logrotate.conf, enable timer |
