# Design Review: 010 — Logging Infrastructure

**Document:**
`docs/cbsd-rs/design/010-20260320T1430-logging-infrastructure.md`

**Cross-referenced against:**
- `cbsd-server/src/ws/dispatch.rs` (log path bug)
- `cbsd-server/src/logs/writer.rs` (actual file path)
- `cbsd-server/src/routes/builds.rs` (tail/full paths)
- `cbsd-server/src/logs/sse.rs` (SSE path from DB)
- `cbsd-server/src/logs/gc.rs` (GC path from DB)
- `cbsd-server/src/config.rs` (LoggingConfig, dead_code)
- `cbsd-server/src/main.rs` (current tracing setup)
- `cbsd-worker/src/main.rs` (current tracing setup)
- `cbsd-server/Cargo.toml` (tracing-appender dep)
- `cbsd-worker/Cargo.toml` (no tracing-appender)
- `tracing-appender` 0.2.4 source (API verification)
- `tracing-subscriber` 0.3.x source (layer API)
- `podman-compose.cbsd-rs.yaml`
- `systemd/cbsd-rs-ctr.sh`

---

## Summary

The design addresses two real issues: a confirmed log path
bug and a genuine gap in file-based logging. Fix 1 is a
correct one-line change. Fix 2 is well-structured with a
clean layered subscriber topology. One blocker: the
`rolling::daily()` file naming pattern will produce
unexpected filenames without the `.log` extension. Two
concerns about the implementation sketch.

**Verdict: Approve with conditions.**

---

## Blockers

### B1 — `rolling::daily()` produces `<prefix>.YYYY-MM-DD`, not `<prefix>.YYYY-MM-DD.log`

The design's implementation sketch uses:

```rust
let appender = tracing_appender::rolling::daily(
    dir, name,
);
```

Where `name` is extracted from the configured `log-file`
path's filename (e.g., `"server.log"`). The `rolling::daily()`
function treats the second argument as a **prefix** — the
produced files will be:

```
server.log.2026-03-20
server.log.2026-03-21
```

Not `server.2026-03-20.log` or `server-2026-03-20.log`.

This is technically functional but operationally confusing:
the files don't have a `.log` extension, which breaks
filename-based log tooling (grep patterns, logrotate globs,
IDE associations).

**Fix:** Use the `RollingFileAppender::builder()` API
instead, which supports `.filename_suffix()`:

```rust
let appender = tracing_appender::rolling::RollingFileAppender
    ::builder()
    .rotation(tracing_appender::rolling::Rotation::DAILY)
    .filename_prefix(stem)     // "server"
    .filename_suffix("log")    // ".log"
    .build(dir)
    .expect("failed to create log appender");
```

This produces `server.2026-03-20.log` — standard, tooling-
friendly file names. The design should specify this builder
pattern and document the expected file naming.

Alternatively, if the `<prefix>.YYYY-MM-DD` naming is
acceptable, document it explicitly so operators know what
filenames to expect.

---

## Major Concerns

### M1 — `file_name()` extraction from config path is fragile

The implementation sketch does:

```rust
let dir = path.parent().unwrap_or(Path::new("."));
let name = path.file_name()
    .and_then(|n| n.to_str())
    .unwrap_or("cbsd.log");
```

If the operator configures `log-file: /cbs/logs/server.log`,
this extracts `dir = /cbs/logs` and `name = server.log`.
The rolling appender then uses `server.log` as the prefix,
producing `server.log.2026-03-20`.

If the operator configures `log-file: /cbs/logs/` (trailing
slash), `file_name()` returns `None` and the fallback
`"cbsd.log"` is used silently — no error.

If the operator configures `log-file: server` (no directory),
`parent()` returns `Some("")` which `unwrap_or` doesn't
catch (it's `Some`, not `None`), and the file is created
relative to the CWD of the process — which in a container
may be `/` or an unexpected location.

**Fix:** Validate the `log-file` path at startup:
- Must be an absolute path (or resolve relative to a known
  base directory).
- Must have a non-empty filename component.
- Parent directory must exist (or be creatable).

### M2 — No old-file cleanup for `rolling::daily()`

The design says "old files accumulate in the configured
directory" and notes "no logrotate configuration" in the
systemd deployment. `rolling::daily()` creates a new file
every day but **never deletes old files**. In a long-running
production deployment, this will fill the disk.

The `tracing-appender` 0.2.x `RollingFileAppender::builder()`
supports `.max_log_files(n)` which automatically removes
files beyond the configured count. The design should either:

1. Use `.max_log_files(30)` (or configurable) in the builder
   to auto-prune, or
2. Document that external logrotate is required and provide
   a logrotate config snippet for the systemd deployment, or
3. Accept unbounded growth and document the operational
   expectation.

Option 1 is simplest and requires no external tooling.

---

## Minor Issues

- **`ansi` feature gate.** The sketch uses
  `.with_ansi(true)` for console output. The `ansi` feature
  is enabled by default in `tracing-subscriber`, but if it
  were ever disabled, `.with_ansi(true)` would not produce
  ANSI output (it's a no-op without the feature, not a
  panic). Verify the feature is present in `Cargo.toml`.
  Currently `tracing-subscriber` has `features = ["env-filter"]`
  but not explicit `ansi` — it's included in the default
  feature set, so this is fine.

- **Worker guard return type.** The sketch returns
  `Option<WorkerGuard>`. When no log file is configured (dev
  mode, console only), the guard is `None` and no file
  appender exists. This is correct — the console layer
  doesn't need a guard (it writes synchronously to stdout).

- **`CBSD_DEV` vs `config.dev.enabled`.** The design uses
  `CBSD_DEV` for logging and `config.dev.enabled` for other
  dev behaviors. This creates two independent dev-mode
  switches. A user could set `dev.enabled: true` in config
  but forget `CBSD_DEV=1`, getting OAuth bypass but no
  console logging. This is acceptable (different concerns)
  but the README section should explicitly note the
  distinction.

- **`EnvFilter` precedence.** The sketch uses
  `EnvFilter::try_from_default_env()` (reads `RUST_LOG`)
  with fallback to the configured `level`. This means
  `RUST_LOG=debug` overrides the config file's
  `level: info`. This is the standard pattern and is
  correct, but operators should know that `RUST_LOG` takes
  priority.

---

## Strengths

- **Fix 1 is surgically correct.** The one-line change
  aligns the DB path with the actual file path. The note
  about stale rows being cleaned up by GC's existing
  `NotFound` handling is accurate and prevents the need
  for a migration.

- **Output mode rules are clear and justified.** Dev gets
  console (always) + file (optional). Prod gets file only.
  Hard error on missing `log-file` in prod prevents silent
  log loss.

- **Decision to duplicate subscriber setup is correct.**
  `cbsd-proto` is zero-IO. Pulling tracing infrastructure
  into it would break the design constraint for ~25 lines
  of savings. The duplication is the right call.

- **`CBSD_DEV` as env var is the right mechanism** for
  both binaries. Config-file-based detection would require
  the worker to parse config before setting up logging,
  creating a chicken-and-egg problem. The env var is
  available immediately at process start.

- **Guard lifetime is correctly specified.** The `let
  _guard = ...;` pattern in `main()` holds the guard for
  the process lifetime. The `#[must_use]` on `WorkerGuard`
  will catch accidental `let _ = ...;` bindings.

- **`tracing-appender` is already a dependency** of
  `cbsd-server`. Only `cbsd-worker` needs the addition.

---

## Suggestions

- **Consider `max_log_files` in the builder.** A default of
  30 daily files (one month of retention) is a reasonable
  default that prevents unbounded disk growth without
  external tooling.

- **Consider making the rotation period configurable.**
  `rolling::daily()` is a sensible default, but some
  deployments may prefer hourly rotation under high log
  volume. The builder supports `Rotation::HOURLY` and
  `Rotation::NEVER`. A `rotation: daily` config field
  with a default of `daily` would future-proof this.

- **The compose file change is minimal** — just adding
  `CBSD_DEV: "1"` to the dev services' environment
  sections. The design correctly notes this.

---

## Open Questions

1. **Should `log-file` accept a directory (auto-naming)
   or only a file path?** The current design uses the
   filename as a prefix. If operators configure a
   directory, the behavior is unexpected. Clarify the
   contract.

2. **Should the `level` config also apply to the file
   layer independently?** Currently both console and file
   share the same `EnvFilter`. A deployment might want
   `info` on console and `debug` in the file. This is a
   future enhancement, not a blocker.
