---
seq: "019"
type: impl
title: security-audit-remediation-key-prefix-index
version: 1
date: 2026-06-11T11:38
commit: 1d259e8a
---

# Impl Review — Security Audit Remediation: api_keys.key_prefix Index (v1)

**Commit:** `1d259e8a` — "cbsd-rs/server: index api_keys.key_prefix for O(log n)
lookups"\
**Plan section:** Commit 16, plan
`019-20260516T1033-security-audit-remediation.md` lines 755–774\
**Design section:** D7 / F10, `019-20260514T1040-security-audit-remediation.md`
lines 721–742\
**Branch:** `wip/cbsd-rs-security-review`

---

## 1. Summary Assessment

This is a clean, minimal, well-executed migration commit. The index name is
correct, the non-unique choice is correctly justified, and the migration is
idempotent. The `.sqlx` cache was regenerated with the right command
(`--all-targets`), no test-target files were pruned, and the single modified
cache file's nullability delta is benign (sqlx SQLite inference quirk,
overridden by the `AS "id!"` suffix). The test is non-vacuous — it asserts
`SEARCH via idx_api_keys_key_prefix` in the EXPLAIN plan, not merely that the
index exists. All 265 workspace tests pass.
`SQLX_OFFLINE=true cargo check --workspace --all-targets` completes clean.

**Verdict: GO.** No blockers. One minor observation on the timing-side-channel
framing; no code changes required.

---

## 2. Strengths

**Correct leading-column analysis.** The commit message and migration comment
accurately diagnose why the existing composite
`UNIQUE (owner_email, key_prefix)` cannot serve a prefix-only lookup: a B-tree
index can only seek on a left-prefix of its key columns, so
`WHERE key_prefix = ?` without `owner_email` cannot use it. SQLite falls back to
a full table SCAN. The new standalone index on `key_prefix` alone closes this.
The analysis is right and the fix is minimal.

**Non-unique index is correct.** Two different full API keys can share the same
12-hex prefix (a UX/lookup helper, not a unique identifier). The migration
comment explains this. Making the index non-unique is the right call and is
consistent with how `robot_tokens.token_prefix` (indexed non-uniquely in
migration 007 via `idx_robot_tokens_prefix`) is handled.

**`CREATE INDEX IF NOT EXISTS`.** The migration is idempotent. Re-running
migrations on an already-migrated database is a no-op rather than an error. The
test verifies this explicitly.

**Genuine EXPLAIN assertion.** The test `key_prefix_index_exists_and_is_used`
goes beyond checking existence: it runs `EXPLAIN QUERY PLAN` and asserts the
`detail` column contains `idx_api_keys_key_prefix`. If the index were dropped or
renamed this test would fail with a clear message. It uses `sqlx::query()`
(non-macro), so it adds no new `.sqlx` cache entry.

**Correct scope.** `robot_tokens.token_prefix` was already indexed by migration
007 (`idx_robot_tokens_prefix`). This commit touching only `api_keys` is
correct; it is not a missed table.

**Cache integrity confirmed.** The parent commit had 140 `.sqlx` files; the
current tree has 140. Only one file was modified.
`SQLX_OFFLINE=true cargo check --workspace --all-targets` (which includes
`#[cfg(test)]` modules where pruned files would cause compilation errors) passes
clean. The cache is complete and consistent.

---

## 3. Blockers

None.

---

## 4. Major Concerns

None.

---

## 5. Minor Issues

**Timing side-channel framing is secondary.** The commit message and migration
comment both foreground the timing side channel as the primary motivation. This
framing, while consistent with the design doc (D7 / F10), overstates the
security impact: Argon2id dominates the wall-clock time for any cache-miss
verification path, so the marginal signal from a table SCAN vs. an index SEARCH
is small in practice. The unambiguous, non-contestable benefits are performance
(O(log n) vs. O(n) growth with key count) and DoS resistance (scan cost per
request is bounded). The timing-channel claim is the design's framing and is
worth preserving for traceability, but leading with "timing side channel" in
commits and comments risks overstating the threat to future readers.

No code change required — this is a framing observation for future commit
messages in this series.

---

## 6. Suggestions

None.

---

## 7. Open Questions

None.

---

## 8. Plan Coverage

| Plan item (commit 16)                             | Status      |
| ------------------------------------------------- | ----------- |
| Non-unique B-tree index on `api_keys(key_prefix)` | Implemented |
| Migration comment with before/after query plan    | Implemented |
| `cargo sqlx prepare --workspace` cache regen      | Implemented |
| Migration apply test (forward + idempotent)       | Implemented |

All plan items are fully implemented. Nothing deferred.

---

## 9. Build Verification

| Check                                                     | Result            |
| --------------------------------------------------------- | ----------------- |
| `SQLX_OFFLINE=true cargo build --workspace`               | PASS              |
| `SQLX_OFFLINE=true cargo clippy --workspace`              | PASS (0 warnings) |
| `cargo test --workspace`                                  | PASS (265 tests)  |
| `SQLX_OFFLINE=true cargo check --workspace --all-targets` | PASS              |
| `key_prefix_index_exists_and_is_used` test                | PASS              |
| All 9 migrations apply cleanly against fresh DB           | PASS              |

`cargo sqlx prepare --workspace --check -- --all-targets` could not be run
directly due to the shell's working-directory constraint (`cargo sqlx` has no
`--manifest-path`). The equivalent guarantee — that no test-target `.sqlx` cache
files were pruned — was confirmed by: (a) `.sqlx` file count unchanged (140 both
before and after), (b) `SQLX_OFFLINE=true cargo check --workspace --all-targets`
compiling clean (a pruned file would fail here with "no cached data for this
query").

---

## 10. Adversarial Probe Results

| #   | Probe                                                                           | Verdict   |
| --- | ------------------------------------------------------------------------------- | --------- |
| 1   | Migration 009: non-unique index on `api_keys(key_prefix)`; idempotent           | PASS      |
| 2   | Lookup closure: `WHERE key_prefix = ?` now SEARCH via index                     | PASS      |
| 2   | Composite UNIQUE cannot serve prefix-only lookup (wrong leading column)         | CONFIRMED |
| 3   | `robot_tokens.token_prefix` already indexed (migration 007)                     | CONFIRMED |
| 4   | `.sqlx` cache: one file modified, zero files pruned, offline check clean        | PASS      |
| 4   | Nullability `false→true` for `id!`: sqlx-SQLite quirk, `!` override applied     | BENIGN    |
| 5   | Test asserts SEARCH-via-index, not merely existence; non-macro (no cache entry) | PASS      |
| 6   | Commit smell test: below 200-LOC floor; plan-justified single capability        | PASS      |
| 7   | Build: 0 errors, 0 warnings, 265 tests pass                                     | PASS      |

---

## 11. Confidence Score

| Item           | Points  | Description |
| -------------- | ------- | ----------- |
| Starting score | 100     |             |
| **Total**      | **100** |             |

No deductions. All plan items implemented, migration correct, test non-vacuous,
cache complete, build and tests clean.

---

## 12. Verdict

**GO.**

This commit is ready to proceed. Migration 009 closes audit-rem D7 / F10 fully
and correctly. No follow-up required.
