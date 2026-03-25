# Design Review: 010 — Logging Infrastructure (v2)

**Document:**
`design/010-20260320T1430-logging-infrastructure.md` (v3)

**Cross-referenced against:**


- `tracing-appender` 0.2.4 source (`rolling::never()`)
- `cbsd-rs/systemd/` (existing templates)
- `cbsd-rs/systemd/install.sh`
- All v1 findings

---

## Summary

All 3 v1 findings are resolved:

- **B1 (file naming):** Switched from `rolling::daily()` to
  `rolling::never()` — the application writes to a single
  stable file. No date suffixes, no prefix/suffix issues.
  logrotate handles rotation externally.
- **M1 (path validation):** `validate_log_path()` enforces
  absolute path, non-empty filename, and parent existence.
- **M2 (no cleanup):** logrotate with `rotate 30` +
  `compress` + `delaycompress` handles retention and
  compression. `copytruncate` avoids signaling the process.

The logrotate approach is the right call — `tracing-appender`
rotation cannot compress, and logrotate is the standard Linux
tool for this. The `copytruncate` + `non_blocking` interaction
is correctly analyzed (microsecond duplication window is the
standard trade-off).

No blockers. No major concerns.

**Verdict: Approved.**

---

## Prior Findings Disposition

| v1 Finding | Status |
|---|---|
| B1 — `rolling::daily()` file naming | Resolved (`rolling::never()`) |
| M1 — Path extraction fragile | Resolved (`validate_log_path()`) |
| M2 — No old-file cleanup | Resolved (logrotate) |

---

## Blockers

None.

---

## Major Concerns

None.

---

## Minor Issues

- **`rolling::never()` second argument is the exact
  filename.** Verified in `tracing-appender` 0.2.4 source:
  with `Rotation::NEVER`, the file is `{dir}/{filename}`
  with no date suffix. `rolling::never("/cbs/logs",
  "server.log")` → `/cbs/logs/server.log`. Correct.

- **systemd timer `RandomizedDelaySec=300` (5 minutes).**
  This avoids simultaneous logrotate runs across
  deployments on the same host. The delay is added to
  the base `OnCalendar=*-*-* 00:00:00`. In a single-
  deployment setup this is harmless. Good practice.

- **Compose deployments don't get logrotate.** The design
  acknowledges this: dev is short-lived with `CBSD_DEV=1`,
  staging relies on operator-provisioned rotation. This
  is acceptable and documented.

- **`delaycompress` keeps yesterday's file uncompressed.**
  This allows `tail -f server.log.1` on the most recent
  rotated file. Correct operational choice.

---

## Strengths

- **`rolling::never()` is the simplest correct approach.**
  The application writes one file. The OS manages lifecycle.
  Clean separation of concerns.

- **`copytruncate` avoids the need for signal handling.**
  No `SIGHUP` or `postrotate` script needed. The process
  keeps its open FD. The microsecond duplication window is
  the standard trade-off, well-documented in the design.

- **logrotate config is generated per deployment.** The
  glob `*/logs/*.log` covers server and worker
  subdirectories. Per-deployment state files avoid
  conflicts with system-wide logrotate.

- **`Persistent=true` on the timer catches missed runs.**
  If the host is rebooted at midnight, logrotate runs on
  the next boot.

- **Path validation is thorough.** Absolute path required,
  non-empty filename, parent directory existence check.
  Clear panic messages.

- **Fix 1 (build log path) remains correct.** One-line
  change, no migration, stale rows handled by GC.

- **Implementation sketch compiles.** The API calls
  (`rolling::never`, `non_blocking`, `fmt::layer()
  .with_ansi().with_writer()`, registry `.with()`) are all
  verified against the locked crate versions.
