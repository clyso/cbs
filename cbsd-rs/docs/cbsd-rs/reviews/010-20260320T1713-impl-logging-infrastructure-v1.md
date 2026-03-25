# Implementation Review: 010 — Logging Infrastructure

**Commits reviewed:**


- `da13205` — docs: design, plan, reviews
- `9255de9` — fix build log path stored in database
- `4596788` — file logging with CBSD_DEV console gating
- `48b2dd1` — logrotate for application logs


**Evaluated against:**

- Design `010-20260320T1430-logging-infrastructure.md` (v3)
- Plan `010-20260320T1600-logging-infrastructure.md`

---

## Summary

The implementation faithfully tracks the approved design
across all 4 commits. The bug fix is surgically correct.
The layered tracing subscriber works as designed. The
logrotate integration follows the existing systemd
template pattern. Path validation catches all three
failure modes (relative, no filename, missing parent).

No blockers. One minor finding about duplicate validation
in `setup_tracing` that's already covered by config
validation. One observation about the worker's tracing
init ordering.

**Verdict: Approved. No findings require changes.**

---

## Design Fidelity

| Requirement | Status |
|---|---|
| Fix dispatch.rs log path | ✓ (Commit 2) |
| `rolling::never()` for stable file | ✓ |
| `non_blocking` + `WorkerGuard` held in `_guard` | ✓ |
| `validate_log_path` (absolute, filename, parent) | ✓ (in config.rs) |
| Prod-requires-log-file panic/error | ✓ |
| `CBSD_DEV` console gating | ✓ |
| `EnvFilter` with `RUST_LOG` precedence | ✓ |
| Server: remove `dead_code`, wire `log_file` | ✓ |
| Worker: add `LoggingConfig`, `tracing-appender` | ✓ |
| `setup_tracing` duplicated (not shared) | ✓ |
| logrotate.conf.in template | ✓ |
| Timer: `Persistent=true`, `RandomizedDelaySec` | ✓ |
| Service: per-deployment `--state` file | ✓ |
| `copytruncate` in logrotate config | ✓ |
| `install.sh`: generate config, enable timer | ✓ |
| Compose: `CBSD_DEV: "1"` for dev services | ✓ |
| README: document `CBSD_DEV` | ✓ |
| Example configs updated | ✓ |

---

## Commit-by-Commit Verification

### da13205 — docs

5 files of documentation. Design v3, plan, and two design
reviews. All match the approved documents. ✓

### 9255de9 — bug fix (1 line)

```diff
-"builds/{}/build.log"
+"builds/{}.log"
```

Surgically correct. Aligns DB path with writer, tail, and
full-download routes. SSE and GC now resolve correctly. ✓

### 4596788 — file logging (~293 lines)


**Server `config.rs`:**

- `#[allow(dead_code)]` removed from `log_file`. ✓
- Validation in `validate()`: `is_dev` check, absolute
  path, filename component, parent directory. ✓
- Server uses `panic!` for validation failures (consistent

  with existing `validate()` pattern). ✓

**Server `main.rs`:**

- `setup_tracing()` function with optional console + file
  layers. ✓
- `rolling::never(dir, filename)` — correct API call. ✓
- `non_blocking()` → `(writer, guard)`. Guard held as
  `let _guard`. ✓
- `registry().with(filter).with(console).with(file).init()`

  — correct combinator pattern. ✓
- Called after `load_config()` (validation runs first). ✓

**Worker `config.rs`:**

- `LoggingConfig` struct with `level` and `log_file`. ✓
- `Default` impl with `level: "info"`. ✓
- Validation in `resolve()`: same 3 checks as server. ✓
- Worker uses `ConfigError::Validation` (returns `Result`,

  not `panic!` — matches existing `resolve()` pattern). ✓
- `logging` field added to both `WorkerConfig` and
  `ResolvedWorkerConfig`. ✓

**Worker `main.rs`:**

- `setup_tracing()` identical to server's version. ✓
- Called before `resolve()` using `raw_config.logging`. ✓
- Resolve error falls back to `eprintln!` (correct —
  tracing may have no output in prod-no-log-file case,
  but that case is caught by validation first). ✓

**Config examples:** Updated with `logging` section and
`CBSD_DEV` documentation. ✓

**Compose:** `CBSD_DEV: "1"` added to dev services. ✓

**README:** Documents `CBSD_DEV`, distinction from
`dev.enabled`, compose behavior. ✓

### 48b2dd1 — logrotate (~104 lines)

**`logrotate.conf.in`:** Template with `{{cbsd_rs_data}}`
and `{{deployment}}` placeholders. Directives: `daily`,
`rotate 30`, `compress`, `delaycompress`, `copytruncate`,
`missingok`, `notifempty`. ✓

**Timer:** `Persistent=true`, `RandomizedDelaySec=300`. ✓

**Service:** `Type=oneshot`, per-deployment `--state` file.
`{{cbsd_rs_data}}` placeholder replaced by `install.sh`
via `sed -i`. ✓

**`install.sh`:** Copies template, resolves placeholders
via `sed`, enables timer. Idempotent checks
(`[[ ! -e ... ]]`). ✓

---

## Observations

- **`setup_tracing` panics on path validation redundantly.**
  `setup_tracing()` in both binaries has `unwrap_or_else`
  panics for missing parent directory and filename. These
  conditions are already validated in `config.rs` before
  `setup_tracing` is called. The panics in `setup_tracing`
  are defense-in-depth — they cannot fire in practice
  because config validation catches them first. Not a bug,
  but the redundant panics could be replaced with
  `expect("validated in config")` to make the invariant
  explicit.

- **Worker tracing init ordering is correct but subtle.**
  The worker calls `setup_tracing(&raw_config.logging...)`
  before `raw_config.resolve()`. This means tracing is
  initialized from the raw (unvalidated) config. The
  logging validation inside `resolve()` runs AFTER tracing
  is already set up. In the error case (prod, no log-file),
  `setup_tracing` creates a subscriber with no outputs,
  then `resolve()` returns an error that's caught by the
  `eprintln!` fallback. This works correctly — the
  logging validation in `resolve()` is early enough that
  no `tracing::` calls happen before it, and the error
  reaches the user via stderr.

- **`LoggingConfig` is duplicated between server and
  worker.** Both `config.rs` files define an identical
  `LoggingConfig` struct. The design explicitly accepted
  this trade-off (cbsd-proto is zero-IO). At ~15 lines
  per struct, the duplication is tolerable.

- **`is_dev_mode()` is not extracted as a named function
  in the server.** The server's `config.rs` inlines the
  `std::env::var("CBSD_DEV")` check. The worker's
  `config.rs` does the same. `setup_tracing()` in both
  binaries also checks `CBSD_DEV` independently. The env
  var is read 2-3 times per binary at startup. This is
  harmless (env var reads are cheap) but a shared
  `is_dev_mode()` helper in each binary's scope would
  reduce the repetition. Minor style point.

- **logrotate `install.sh` idempotency.** The `[[ ! -e ]]`
  guards mean re-running `install.sh` on an existing
  deployment won't overwrite a customized logrotate config.
  This is correct — operators who tuned retention or
  rotation frequency won't lose their changes.

---

## No Findings Requiring Changes

The implementation is correct, matches the design, uses
idiomatic Rust patterns (layered subscribers, non-blocking
appender, guard lifetime), and the systemd integration
follows the existing deployment template pattern. The
deliberate code duplication between binaries is justified
and documented. Ready to proceed to the implementation
review commit (plan Commit 5).
