# Plan Review: 010 — Logging Infrastructure

**Plan:**
`plans/010-20260320T1600-logging-infrastructure.md`

**Design:**
`design/010-20260320T1430-logging-infrastructure.md` (v3)

---

## Summary

The plan faithfully tracks the approved v2 design. The
5-commit breakdown is well-reasoned: docs, bug fix, main
feature, systemd integration, impl reviews. Dependencies
flow forward. Each commit is independently useful.

One concern about Commit 3's scope boundary.

**Verdict: Approve with conditions.**

---

## Design Fidelity

| Design requirement | Plan coverage |
|---|---|
| Fix `dispatch.rs` log path format | ✓ Commit 2 |
| `rolling::never()` for stable file | ✓ Commit 3 |
| `non_blocking` + `WorkerGuard` | ✓ Commit 3 |
| `validate_log_path()` (abs, filename, parent) | ✓ Commit 3 |
| `CBSD_DEV` console gating | ✓ Commit 3 |
| Prod-requires-log-file validation | ✓ Commit 3 |
| Server: remove `dead_code`, wire `log_file` | ✓ Commit 3 |
| Worker: add `LoggingConfig`, `tracing-appender` | ✓ Commit 3 |
| `server.yaml.example` update | ✓ Commit 3 |
| `worker.yaml.example` add `logging` section | ✓ Commit 3 |
| `podman-compose`: `CBSD_DEV` for dev services | ✓ Commit 3 |
| `README.md`: document `CBSD_DEV` | ✓ Commit 3 |
| logrotate config template | ✓ Commit 4 |
| systemd timer + service templates | ✓ Commit 4 |
| `install.sh` generates config, enables timer | ✓ Commit 4 |
| `copytruncate` for zero-signal rotation | ✓ Commit 4 |
| `missingok`, `notifempty`, `delaycompress` | ✓ Commit 4 |
| Per-deployment state file | ✓ Commit 4 |
| Design/plan/review docs committed | ✓ Commits 1, 5 |

---

## Commit Breakdown Assessment

### Commit 1 — docs only

Docs-only commits are meaningful checkpoints. The plan
correctly includes all design reviews. The glob
`010-*-plan-logging-infrastructure-*.md` covers this
review itself. ✓

### Commit 2 — bug fix (~5 lines)

The one-line fix is surgically correct and independently
valuable. Separating it from the logging feature allows
cherry-picking and clean `git bisect`. The verification
note ("no sqlx cache update needed") is correct — the
query binds `log_path` as a parameter, not a literal. ✓

### Commit 3 — main feature (~400 lines)

Within the 400-800 target. All changes are tightly
coupled: the subscriber setup needs `CBSD_DEV` detection,
which needs config validation, which needs the config
fields. The compose and README changes are documentation
of the behavior introduced in the same commit. ✓

### Commit 4 — logrotate (~150 lines)

Below 200 but independently meaningful: it adds external
rotation for the file logging introduced in Commit 3.
After Commit 3, logs work but grow unbounded. After
Commit 4, they're rotated and compressed. Clean
"library + consumer" split. ✓

### Commit 5 — impl reviews (docs only)

Same pattern as Commit 1. ✓

---

## Concern

### C1 — Commit 3 changes both binaries + compose + README

Commit 3 touches 9 files across `cbsd-server`,
`cbsd-worker`, `config/`, compose, and README. At ~400
lines this is within the guideline, but the blast radius
is wide. Consider whether the worker changes (Cargo.toml


+ config.rs + main.rs) could be a separate commit after
the server changes.

However: the compose file references both server-dev and
worker-dev services with `CBSD_DEV`. If the worker isn't
updated in the same commit, running the compose dev
profile will fail (worker doesn't recognize
`logging.log-file`). The changes are genuinely coupled
across both binaries via the shared compose file.

**Verdict on C1:** The coupling via compose justifies
keeping both binaries in one commit. The wide blast
radius is acceptable given the tight coupling.

---

## Minor Notes
+
+ **Commit 2 verification says "cargo test --workspace".**
  There are no tests that exercise the log path format
  string — the verification relies on manual testing or
  integration testing. The `cargo test` line is correct
  (it runs existing tests to ensure no regressions) but
  won't catch a typo in the format string itself.
+
+ **Commit 4's `install.sh` changes.** The plan says
  "enable the timer" — verify this uses `systemctl --user
  enable` (not `start`), matching the existing pattern for
  the service target.
+
+ **Commit ordering is correct.** Commit 2 (bug fix) has
  no dependency on Commit 3 (logging). Commit 4 (logrotate)
  depends on Commit 3 (the log files it rotates). Commits
  1 and 5 are documentation bookends. No circular
  dependencies.

---

## No Blockers Found

The plan is a faithful, well-structured translation of the
approved design into 5 commits with correct ordering and
justified granularity.
