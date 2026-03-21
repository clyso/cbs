# Design Review: 011 — Build Artifact Reporting (v2)

**Document:**
`design/011-20260321T0401-build-artifact-reporting.md`
(v2 — addresses review v1)

---

## Summary

All 3 v1 blockers and all 5 v1 major concerns are
resolved. The report file path is corrected to
`/runner/scratch/`, the Rust extraction code is shown
in full, `BuildRecord` changes are specified, the
`skipped` path is explicitly minimal, the runner read
placement is before `rc != 0`, `rpm_count` is removed,
the 64 KB size limit is enforced at the worker, and
signature changes are listed.

No blockers. No major concerns.

**Verdict: Approved.**

---

## Prior Findings Disposition

| v1 Finding | Status |
|---|---|
| B1 — Report file path `/tmp/` not mounted | Resolved (`/runner/scratch/`) |
| B2 — `output.rs` extraction missing | Resolved (code shown) |
| B3 — `BuildRecord`/`list_builds` gaps | Resolved (specified) |
| M1 — `skipped` path underspecified | Resolved (minimal report) |
| M2 — `runner.py` read placement | Resolved (before `rc` check) |
| M3 — `rpm_count` no data source | Resolved (removed) |
| M4 — No size limit | Resolved (64 KB at worker) |
| M5 — Signature changes unlisted | Resolved (listed) |

---

## Blockers

None.

---

## Major Concerns

None.

---

## Minor Issues

- **Partial reports on failure are deferred.** The design
  correctly places the file read before the `rc` check
  but notes the `RunnerError` exception path loses the
  report. A future iteration could attach the report to
  the exception. Acceptable deferral — the design is
  honest about it.

- **`report.py` placement.** New file
  `cbscore/src/cbscore/builder/report.py`. The existing
  `cbscore/releases/desc.py` has closely related types.
  `ComponentReport` is a projection of
  `ReleaseComponentVersion`. Consider importing from
  `desc.py` to avoid structural drift, or note that the
  two representations intentionally diverge.

- **`list_builds` excludes `build_report`.** The design
  says the list endpoint uses "a separate query or
  response type." This is the right call for performance
  but the exact mechanism (separate struct vs
  `#[serde(skip)]` vs different SQL) should be decided
  during implementation. Not a design concern.

- **`serde_json::Value` vs `RawValue`.** The design uses
  `Value` in the worker. `serde_json::value::RawValue`
  would avoid full parse-and-reserialize (the worker is
  a pass-through). Minor optimization — not required.

---

## Strengths

- **All 8 v1 findings resolved** with concrete, correct
  fixes. No regressions.
- **Report file path is now correct** —
  `/runner/scratch/` is bind-mounted and verified.
- **`finally` block for cleanup** prevents stale reports.
- **64 KB size limit at the worker** with code shown.
- **`report_version: 1`** for future schema evolution.
- **`skipped` path is explicitly minimal** — no S3
  lookup, just container image info.
- **Runner read placement before `rc` check** captures
  partial reports on success.
- **`list_builds` excludes report** — prevents expensive
  list responses.
- **`row_to_build_record` deserializes TEXT→Value** — API
  response shows nested object, not quoted string.
- **Compact JSON contract documented** — wrapper emits
  `separators=(",",":")`, Rust detection hardcoded.
- **Backwards compatibility genuinely correct** — each
  layer deployable independently.
- **Migration named `004_build_report.sql`** — follows
  convention.
