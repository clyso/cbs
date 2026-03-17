# Implementation Review: cbsd-rs Phase 8 — Compile-Time Checked SQL Queries

**Commit reviewed:**
- `5be2e5f` — migrate to sqlx compile-time checked query macros (1924+, 443−, 81 files)

**Evaluated against:**
- Design: `_docs/cbsd-rs/design/2026-03-17-sqlx-compile-time-queries.md`
- Plan: `_docs/cbsd-rs/plans/phase-8-sqlx-macros.md`

---

## Summary

Phase 8 is a clean, mechanical migration delivered as a single atomic
commit. All 76 eligible queries across 13 source files are migrated from
`sqlx::query()` to `sqlx::query!()`. The 1 dynamic query (`list_builds`)
correctly remains as `sqlx::query()`. The `.sqlx/` offline cache (67 JSON
files) is committed alongside the source changes, enabling
`SQLX_OFFLINE=true` builds immediately.

No logic changes. No behavioral differences. Every query does exactly
what it did before — just validated at compile time.

**No blockers. No findings. Implementation is clean.**

**Verdict: Approved. Ready to merge.**

---

## Design Fidelity

| Design requirement | Status |
|---|---|
| `"macros"` added to sqlx features in Cargo.toml | ✓ |
| 76 queries migrated to `query!()` across 13 files | ✓ (verified: 76 `query!` calls, 1 `query(`) |
| 1 dynamic query (`list_builds`) stays as `sqlx::query()` | ✓ (`builds.rs:122`) |
| `.sqlx/` offline cache committed | ✓ (67 JSON files) |
| `db/tokens.rs` — 4 queries | ✓ |
| `db/users.rs` — 3 queries | ✓ |
| `db/api_keys.rs` — 8 queries | ✓ |
| `db/workers.rs` — 7 queries | ✓ |
| `db/builds.rs` — 9 of 10 queries | ✓ |
| `db/roles.rs` — 21 queries | ✓ |
| `db/seed.rs` — 5 queries | ✓ |
| `queue/recovery.rs` — 7 queries | ✓ |
| `routes/admin.rs` — 6 queries | ✓ |
| `routes/permissions.rs` — 2 queries | ✓ |
| `logs/gc.rs` — 2 queries | ✓ |
| `logs/writer.rs` — 1 query | ✓ |
| `logs/sse.rs` — 1 query | ✓ |
| Transaction queries use `&mut **tx` / `&mut *tx` | ✓ |
| `Row` import removed from all files except `builds.rs` | ✓ |
| `row_to_build_record` retained for dynamic query | ✓ |
| Single atomic commit (feature + migration + cache) | ✓ |

---

## Per-File Verification

### `db/tokens.rs` (4 queries)

All 4 DML queries migrated. Field access via `.revoked` instead of
`.get("revoked")`. Clean — simplest module, no transactions.

### `db/users.rs` (3 queries)

`get_user` uses `AS "email!", "name!", active` type overrides. The
`active` column is nullable in sqlx's view (SQLite type affinity), so
accessing `r.active != 0` on the macro-generated `i32` field is correct.
`create_or_update_user` uses `sqlx::Error::RowNotFound` — unchanged
from before.

### `db/api_keys.rs` (8 queries)

Both pool and transaction variants migrated. `find_api_keys_by_prefix`
uses extensive `AS "col!"` overrides for all NOT NULL columns — correct
because sqlx's SQLite driver cannot always infer non-nullability from
WHERE predicates. `insert_api_key_in_tx` passes `&mut **tx` — correct
for `Transaction<'_, Sqlite>`.

### `db/workers.rs` (7 queries)

All 3 SELECT queries use `AS "col!"` overrides on NOT NULL columns.
`last_seen` (nullable) has no override — correct, macro infers
`Option<i64>`. Transaction queries use `&mut **tx`. Clean.

### `db/builds.rs` (9 migrated + 1 dynamic)

`insert_build` uses `RETURNING id AS "id!"` — correct for getting the
auto-generated ID. `get_build` has full `AS "col!"` overrides for NOT
NULL columns, nullable columns (`worker_id`, `trace_id`, `error`,
`started_at`, `finished_at`) left undecorated — correct. `list_builds`
and `row_to_build_record` remain unchanged with `sqlx::query()` +
`Row::get()`. The `use sqlx::{Row, SqlitePool}` import is correctly
retained.

### `db/roles.rs` (21 queries)

Largest module. All queries migrated. Notable patterns:

- `create_role`: `builtin` param converted to `i32` before binding
  (`let builtin_int = builtin as i32`) — correct, sqlx macro requires
  the concrete type, not `bool`.
- `count_active_wildcard_holders`: uses `AS "cnt!"` on the COUNT result.
  Return type is `i64` (via `.cnt.into()`). Previously used
  `sqlx::Row::get`. Correct.
- `has_assignments`: uses `EXISTS(...)  AS "has_any!"`. Result is `i32`,
  compared `!= 0`. Correct.
- Loop queries in `set_user_roles` and `get_user_assignments_with_scopes`
  correctly use `query!()` inside loops — each invocation gets its own
  anonymous struct type. No performance regression (same SQL text,
  same bind count).

### `db/seed.rs` (5 queries)

All queries inside the transaction block migrated. The `COUNT(*) AS
"cnt!"` pattern for the empty-check is consistent with `roles.rs`.
Dev worker seeding queries (in the loop) use `query!()` correctly.

### `queue/recovery.rs` (7 queries)

All 7 startup recovery queries migrated. Notable: the nested
`sqlx::query!()` inside the error path (corrupt descriptor → mark
failure) correctly uses `query!()` too. The `SELECT ... FROM builds
WHERE state IN ('dispatched', 'started')` works because the IN clause
contains string literals (not bind parameters).

### `routes/admin.rs` (6 queries)

All 6 inline SQL queries migrated. The `deactivate_user` handler's
admin-count query uses `AS "cnt!"` — previously extracted via
`sqlx::Row::get(&row, "cnt")`. Now directly accessed as `.cnt`. The
intermediate `let count` + `let row` pattern is collapsed to
`.fetch_one(...).await?.cnt` — cleaner. Both `api_keys SET revoked = 1`
queries (deregister + regenerate) use `query!()`.

### `routes/permissions.rs` (2 queries)

`list_users_with_roles` uses `AS "email!", "name!", "active!"` — correct.
`add_user_role` scope insertion uses `query!()` with `INSERT OR IGNORE`.
Both migrated correctly.

### `logs/gc.rs` (2 queries)

The GC SELECT query uses `AS "build_id!", "log_path!"` — correct. The
DELETE query uses `query!()`. Clean.

### `logs/writer.rs` (1 query)

`UPDATE build_logs SET log_size = ?` migrated. Clean.

### `logs/sse.rs` (1 query)

`SELECT log_path, finished FROM build_logs` with `AS "log_path!",
"finished!"`. Result mapped to `BuildLogRow`. Clean.

---

## `.sqlx/` Offline Cache

67 cache files for 76 `query!()` calls. The delta (76 − 67 = 9) is
expected: sqlx deduplicates by query text hash. Queries with identical
SQL (e.g., the two `UPDATE api_keys SET revoked = 1 WHERE id = ? AND
revoked = 0` in `admin.rs`) share a single cache entry.

Spot-checked one file: valid format with `db_name: "SQLite"`, query
text, column/parameter descriptors, and SHA-256 hash.

---

## Observations

- **No `query_as!()` used.** The design and plan both mention preferring
  `query_as!` for SELECTs mapping to existing structs. The implementation
  uses `query!()` with manual field mapping everywhere. This is a valid
  approach — it avoids the sqlx `FromRow` derive requirement and keeps
  the struct definitions unchanged. The manual mapping is slightly more
  verbose but doesn't affect correctness or maintainability at this
  scale. Not a concern.

- **Consistent `AS "col!"` pattern.** Every NOT NULL column in a SELECT
  query that sqlx can't infer as non-nullable (which is most of them for
  SQLite) uses the `AS "col!"` type override. This is the correct
  approach for SQLite where the driver's type inference is limited. The
  pattern is applied uniformly across all 13 files.

- **`bool` handling is correct throughout.** SQLite stores booleans as
  integers. The migration correctly maintains `!= 0` comparisons on
  integer fields (`active`, `revoked`, `builtin`, `finished`) rather
  than treating them as native booleans.

- **`Row` import cleanup is complete.** Only `db/builds.rs` retains
  `use sqlx::Row` — exactly where it's needed for the dynamic query.
  All other files removed it.

- **Single commit at ~1200 LOC.** Exceeds the 400–800 guideline but the
  plan explicitly justifies this: splitting would create intermediate
  commits that don't build without `DATABASE_URL`. The mechanical nature
  of the changes makes the larger commit reviewable.
