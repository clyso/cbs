# 010 — Logging Infrastructure: Implementation Plan

**Design:**
`docs/cbsd-rs/design/010-20260320T1430-logging-infrastructure.md`

## Commit Breakdown

5 commits, ordered by dependency.

### Commit 1: `cbsd-rs/docs: add logging infrastructure design and plan`

**Documentation only**

Add the design document, implementation plan, and all
design/plan reviews produced during the design phase.

**Files:**

| File | Change |
|------|--------|
| `docs/cbsd-rs/design/010-20260320T1430-logging-infrastructure.md` | Design document (v3, approved) |
| `docs/cbsd-rs/plans/010-20260320T1600-logging-infrastructure.md` | This implementation plan |
| `docs/cbsd-rs/reviews/010-20260320T1506-design-logging-infrastructure-v1.md` | Design review v1 (approve with conditions) |
| `docs/cbsd-rs/reviews/010-20260320T1606-design-logging-infrastructure-v2.md` | Design review v2 (approved) |
| `docs/cbsd-rs/reviews/010-*-plan-logging-infrastructure-*.md` | Plan review(s), if produced before this commit |

**Verification:**

- Files are well-formed Markdown.

---

### Commit 2: `cbsd-rs/server: fix build log path stored in database`

**~5 authored lines**

Fix the path mismatch between what dispatch stores in the DB
(`builds/{id}/build.log`) and what the writer actually creates
(`builds/{id}.log`). The SSE follow handler and log GC both
read from the DB and currently look in the wrong place.

**Files:**

| File | Change |
|------|--------|
| `cbsd-server/src/ws/dispatch.rs` | Change `format!("builds/{}/build.log", ...)` to `format!("builds/{}.log", ...)` |

**Verification:**

- `SQLX_OFFLINE=true cargo build --workspace`
- `cargo test --workspace`
- No sqlx cache update needed (the query uses a bind
  parameter for `log_path`, not the format string)

---

### Commit 3: `cbsd-rs: add file logging with CBSD_DEV console gating`

**~400 authored lines**

Wire up `tracing-appender` for file-based logging in both
binaries. Console output is gated on `CBSD_DEV=1`. Production
mode (no `CBSD_DEV`) requires `logging.log-file` or the binary
refuses to start. Update configuration examples, compose file,
and README to document the new behavior.

Uses `tracing_appender::rolling::never()` — the application
writes to a single stable file. Rotation is handled by
logrotate on the host (commit 4).

**Files — Rust code:**

| File | Change |
|------|--------|
| `cbsd-server/src/config.rs` | Remove `#[allow(dead_code)]` from `log_file`. Add `is_dev_mode()` check to `validate()`: if `!is_dev && log_file.is_none()`, panic. |
| `cbsd-server/src/main.rs` | Replace `tracing_subscriber::fmt().with_env_filter(filter).init()` with layered subscriber: registry + optional console layer (CBSD_DEV) + optional file layer (log_file). Hold `_guard`. |
| `cbsd-worker/Cargo.toml` | Add `tracing-appender = "0.2"` dependency. |
| `cbsd-worker/src/config.rs` | Add `LoggingConfig` struct with `level: String` and `log_file: Option<PathBuf>`. Add `logging: LoggingConfig` field to `WorkerConfig` with `#[serde(default)]`. Add validation in `resolve()`: if `!is_dev && log_file.is_none()`, return error. |
| `cbsd-worker/src/main.rs` | Replace `tracing_subscriber::fmt()...init()` with layered subscriber matching the server pattern. Use `config.logging.level` instead of hardcoded `"info"`. Hold `_guard`. |

**Files — configuration and documentation:**

| File | Change |
|------|--------|
| `config/server.yaml.example` | Update `logging:` section: document that `log-file` is required in production (no `CBSD_DEV`), add example path, document `CBSD_DEV` interaction. |
| `config/worker.yaml.example` | Add `logging:` section with `level` and `log-file` fields. |
| `podman-compose.cbsd-rs.yaml` | Add `CBSD_DEV: "1"` to the `environment:` section of `server-dev` and `worker-dev` services. |
| `cbsd-rs/README.md` | Add "Logging" subsection under Development: document `CBSD_DEV`, distinction from `dev.enabled`, compose behavior. |

**Key implementation details:**

- `is_dev_mode()` is a free function in each binary's
  main.rs (not shared — cbsd-proto is zero-IO).
- `validate_log_path()` checks: absolute path, non-empty
  filename, parent directory existence. Panics on failure.
- The `WorkerGuard` is stored as `let _guard = ...;` in
  `main()` — dropped at process exit, flushing the buffer.
- Server validation happens in `ServerConfig::validate()`
  (called before tracing init, so the panic goes to stderr).
- Worker validation happens in `WorkerConfig::resolve()`
  which returns `Result` — the error is printed to stderr
  before tracing is initialized.

**Verification:**

- `SQLX_OFFLINE=true cargo build --workspace`
- `cargo test --workspace`
- Manual: run server with `CBSD_DEV=1` and no `log-file` →
  console output appears.
- Manual: run server without `CBSD_DEV` and no `log-file` →
  startup panic with clear message.
- `podman-compose -f podman-compose.cbsd-rs.yaml config
  --profile dev` parses without error.

---

### Commit 4: `cbsd-rs/systemd: add logrotate for application logs`

**~150 authored lines**

Add logrotate integration for the systemd deployment. A
per-deployment logrotate config is generated by `install.sh`
and run daily via a systemd user timer.

**Files:**

| File | Change |
|------|--------|
| `systemd/templates/systemd/cbsd-rs-logrotate@.timer` | New file: daily timer with `Persistent=true`, `RandomizedDelaySec=300`. |
| `systemd/templates/systemd/cbsd-rs-logrotate@.service` | New file: oneshot service running `/usr/sbin/logrotate` with per-deployment state file and config. |
| `systemd/templates/config/logrotate.conf.in` | New file: logrotate config template with `{{cbsd_rs_data}}` and `{{deployment}}` placeholders. Directives: `daily`, `rotate 30`, `compress`, `delaycompress`, `copytruncate`, `missingok`, `notifempty`. |
| `systemd/install.sh` | After installing per-deployment service/target: generate `logrotate.conf` from template (resolve placeholders), install timer+service templates (if not already present), enable the timer. |

**Key implementation details:**

- `logrotate.conf` is generated at
  `${data_dir}/${deployment}/logrotate.conf` with the glob
  `${data_dir}/${deployment}/*/logs/*.log` covering both
  server and worker log directories.
- The logrotate state file is at
  `${data_dir}/logrotate.${deployment}.state` to avoid
  conflicting with the system-wide state.
- The timer/service templates use `%i` (systemd instance
  name) = deployment name, matching the existing pattern.
- `install.sh` installs and enables the timer once per
  deployment, unconditionally (logrotate is a no-op when
  no log files exist due to `missingok`).

**Verification:**

- `./cbsd-rs/systemd/install.sh --help` still works.
- Inspect generated `logrotate.conf` for correct paths.
- `systemctl --user list-timers` shows the logrotate timer
  after installation.

---

### Commit 5: `cbsd-rs/docs: add implementation reviews for logging infrastructure`

**Documentation only**

Add implementation review documents produced after commits
2–4 are complete.

**Files:**

| File | Change |
|------|--------|
| `docs/cbsd-rs/reviews/010-*-impl-logging-infrastructure-*.md` | Implementation review(s) |

**Verification:**

- Files are well-formed Markdown.

---

## Progress

| # | Commit | Status |
|---|--------|--------|
| 1 | Design, plan, reviews | Done |
| 2 | Fix build log path | Done |
| 3 | File logging + CBSD_DEV + configs + docs | Done |
| 4 | Logrotate systemd | Done |
| 5 | Implementation reviews | Pending |
