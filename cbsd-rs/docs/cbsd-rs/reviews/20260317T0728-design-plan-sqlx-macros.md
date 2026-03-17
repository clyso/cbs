# Design & Plan Review: Compile-Time Checked SQL Queries (Phase 8)

**Documents reviewed:**
- `_docs/cbsd-rs/design/2026-03-17-sqlx-compile-time-queries.md`
- `_docs/cbsd-rs/plans/phase-8-sqlx-macros.md`

**Cross-referenced against:** actual `sqlx::query(` counts in the codebase.

---

## Summary

The design and plan are well-structured for a mechanical migration that
eliminates an entire class of runtime SQL errors. The approach is correct:
file-by-file migration, macro and string APIs coexist, `.sqlx/` cache
committed as the final step. The commit sizing is appropriate and the
dependency graph is accurate.

One blocker: `queue/recovery.rs` (7 queries) is missing from both the
design and the plan. One minor count discrepancy in `routes/admin.rs`.
No architectural concerns.

**Verdict: Approve with conditions.** Add the missing file to the plan
and correct the counts.

---

## Blocker

### B1 — `queue/recovery.rs` (7 queries) not in scope

The design lists 77 queries across 12 files. The actual codebase has
**77 queries across 13 files**. The missing file is
`cbsd-server/src/queue/recovery.rs` with 7 queries:

```
recovery.rs:36:  SELECT id FROM builds WHERE state IN ('dispatched', 'started')
recovery.rs:43:  UPDATE builds SET state = 'failure', error = ..., finished_at = ...
recovery.rs:53:  SELECT id FROM builds WHERE state = 'revoking'
recovery.rs:60:  UPDATE builds SET state = 'revoked', finished_at = ...
recovery.rs:65:  UPDATE build_logs SET finished = 1, updated_at = ...
recovery.rs:74:  SELECT id, descriptor, priority, user_email, queued_at FROM builds WHERE state = 'queued'
recovery.rs:105: UPDATE builds SET state = 'failure', error = 'corrupt descriptor', finished_at = ...
```

These 7 queries are not listed in the design's scope table, not assigned
to any plan commit, and will remain as unchecked `sqlx::query()` calls
after Phase 8 completes if not addressed.

**Fix:** Add `queue/recovery.rs` (7 queries) to the plan. Natural home:
Commit 7 (which already covers `db/seed.rs` and inline SQL — both are
startup-path code like recovery). Alternatively, create a dedicated
Commit 7b. Update the design's scope table to show 13 files and correct
the total (76 migrated + 1 dynamic = 77, or adjust once recovery is
included: 83 migrated + 1 dynamic = 84).

Wait — the actual total is 77 including recovery's 7. The design says
77 but only lists 70 in the table (sum the table: 21+10+8+7+5+4+3+5+2+2+1+1 = 69,
not 77). The table undercounts by 8: 7 from the missing `recovery.rs`
and 1 from `admin.rs` (actual: 6, listed: 5).

**Corrected totals:**
- 13 files, 77 queries total
- 76 to migrate (77 - 1 dynamic `list_builds`)
- The plan's progress table says "69 queries migrated" which is also
  wrong (should be 76)

---

## Minor Issues

- **`routes/admin.rs` count: design says 5, actual is 6.** The 6th query
  is the second `UPDATE api_keys SET revoked = 1` in
  `regenerate_worker_token` (line 526). The plan's Commit 7 allocates
  5 queries for this file. Update to 6.

- **Plan totals are inconsistent.** The progress table header says
  "69 queries migrated (+ 8 already `.execute()` queries that only need
  `query!` swap)." The 69+8 = 77 interpretation doesn't match the
  per-commit query counts (which sum to 69 in the table). The "+8" note
  is confusing — all 77 queries use `sqlx::query()` and need the same
  mechanical change. Drop the distinction and state: "76 queries migrated
  to `query!()`, 1 dynamic query stays as `sqlx::query()`."

- **Intermediate commits don't compile with `SQLX_OFFLINE=true`.** The
  plan notes this correctly ("developers have `DATABASE_URL` set"). But
  CI will fail on Commits 1–8 if it uses `SQLX_OFFLINE=true` (which it
  should, per the CLAUDE.md rule). Either: (a) temporarily set
  `SQLX_OFFLINE=true` with no `.sqlx/` cache (builds with string API
  queries still compile), or (b) accept that CI may need `DATABASE_URL`
  during this phase. The plan's "Notes" section partially addresses this
  but should be explicit about the CI configuration during the migration
  window.

- **`query_as!` guidance is correct but could be more prescriptive.** The
  plan says "use `query!()` for most queries" and `query_as!` "only when
  the result needs to be returned as an existing named struct." In
  practice, many SELECT queries currently return via helper functions
  like `row_to_build_record()` that map to existing structs
  (`BuildRecord`, `WorkerRow`, etc.). These are natural candidates for
  `query_as!` — it would eliminate the mapping code entirely. The plan
  should note this as a simplification opportunity, not just a fallback.

---

## Strengths

- **Correct migration strategy.** File-by-file, independent commits,
  macro and string API coexist. No big-bang commit.
- **The 1 dynamic query exception is correctly identified** and
  documented. `list_builds` with runtime WHERE construction is the only
  query that genuinely cannot use the macro.
- **Commit sizing is appropriate.** Largest commit (Commit 6, ~300 lines
  for 21 queries in `roles.rs`) is at the upper end but acceptable for
  mechanical changes with minimal logic.
- **Dependency graph is clean.** Commit 1 → [2–8 independent] → 9.
- **The plan correctly separates the developer workflow** ("modifying
  queries" needs `DATABASE_URL`) from the consumer workflow ("building"
  uses `SQLX_OFFLINE=true`).

---

## Open Questions

- **Should `list_builds` be refactored to avoid dynamic SQL?** The
  function constructs WHERE clauses at runtime for optional filters
  (`user_email`, `state`). An alternative: use separate named queries
  for each filter combination (4 variants: no filter, user only, state
  only, both). Each would be a `query!()` call. This eliminates the
  last unchecked query at the cost of 3 extra query definitions. Worth
  considering but not blocking for Phase 8.
