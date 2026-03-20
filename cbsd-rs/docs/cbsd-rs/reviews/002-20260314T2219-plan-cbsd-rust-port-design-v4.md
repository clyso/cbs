# Plan Review: cbsd-rs Implementation Plans v4 (Phases 0–6)

**Plans reviewed:**
- `_docs/cbsd-rs/plans/README.md` through `phase-6-integration.md` (all 9 files)

**Cross-referenced against:**
- `_docs/cbsd-rs/design/` (all 4 design documents)

---

## Summary

The plans are clean. All blockers and major concerns from the prior three review passes have been resolved: tower-sessions init, HKDF crate, watch channel lifecycle, `insert_build_log_row` attribution, Stopping+ack-timer test case, shutdown log-drain ordering, `build_logs.finished` in recovery, and all missing crate dependencies (uuid, clap, tokio-util, hkdf). The commit ordering is correct, the testable outcomes are honest, the 8a/8b dispatch split is clean, and the design-to-plan traceability is strong.

No blockers. No major concerns. Six minor observations remain — all implementation notes, not design flaws. The most notable is M1: `serde_json` serializes struct fields in declaration order (not alphabetical), so the PASETO canonical JSON form depends on the `CbsdTokenPayloadV1` struct having fields in alphabetical order by coincidence. A code comment or `BTreeMap`-based serialization would make this invariant explicit.

**Verdict: Approve — ready to implement. Start Phase 0.**

---

## Blockers

None.

---

## Major Concerns

None.

---

## Minor Issues

### M1 — PASETO canonical JSON relies on struct field declaration order, not guaranteed alphabetical

`serde_json::to_string` serializes struct fields in declaration order. The frozen payload `{"expires":...,"user":...}` is alphabetical because the struct declares `expires` before `user`. If a future field (e.g., `iat`) is added between them in the wrong declaration position, the hash invariant breaks silently.

**Fix:** Add a code comment on `CbsdTokenPayloadV1`: "Fields MUST remain in alphabetical order — serde_json serializes in declaration order and SHA-256 of this struct is part of the token storage contract." Alternatively, use a `BTreeMap<&str, Value>` for the canonical serialization path. Also ensure the cross-language hash test is in CI checks in `cbsd-rs/CLAUDE.md`.

### M2 — "Log endpoints return 404 for QUEUED builds" note is under Commit 6 but log routes don't exist until Commit 9

The note is correct but placed under the wrong commit. Move to Commit 9 or add "(tested in Commit 9)" to the Commit 6 text.

### M3 — `GET /admin/queue` "active builds" portion depends on Phase 4's `ActiveBuild`

Commit 6 adds the endpoint, but `ActiveBuild` and the `active` map are defined in Commit 7. Note "active builds section returns empty until Phase 4" or defer the full endpoint to Commit 7.

### M4 — Phase 5 parallelism note not reflected in README dependency graph

The parallelism note (Commit 10 can be developed alongside Phase 4) is in Phase 5's file but the README says "Phase 5 depends on Phase 4." Add a one-sentence qualifier to the README: "Commit 10 compilation has no Phase 4 dependency; full integration testing requires Phase 4 complete."

### M5 — Log-size periodic flush timer `JoinHandle` not tracked for shutdown

Commit 9's 5-second `log_size` flush timer should have its `JoinHandle` stored in `AppState` alongside the GC and sweep handles, or explicitly note that stale `log_size` on shutdown is acceptable.

### M6 — `ApiKeyCache.insert()` clones entire `CachedApiKey` before `push()`

The eviction pattern clones the entry before push to keep it available for reverse-map inserts. One unnecessary clone per cache miss. Negligible at CBS load. Worth a code comment explaining why.

---

## Suggestions

- **Resolve `glob-match` `*` path separator behavior before Commit 5.** Add a one-line resolution note: either "verified: `*` does not match `/`" or "verified: `*` matches `/` — patterns documented accordingly."
- **Recovery step 4 "drop stale watch senders" is a no-op on fresh startup.** Reframe as `assert!(log_watchers.is_empty())` or remove.
- **`cbscore-wrapper.py` test location.** Add `CBSD_WRAPPER_PATH` env var override in Commit 11 so integration tests can locate the script without container image machinery.
- **`DELETE /api/auth/api-keys/{prefix}` when two users share a prefix.** The endpoint takes only `{prefix}` in the path. How does the server identify which owner's key to delete? Resolve before Commit 4 — either add `?owner=<email>` for admin callers or document that non-admin deletions are always scoped to the caller.
- **`trace_id` on re-dispatch.** If a build is re-queued (ack timeout, send failure) and dispatched again, does the DB `builds.trace_id` update to the new UUID? If not, log correlation breaks for multi-attempt dispatches. Decide before Commit 8a.
- **Components filesystem scan — startup cache or per-request?** Commit 6 doesn't specify. A startup cache with optional reload is reasonable. Decide before implementation.

---

## Strengths

- **All prior blockers resolved.** tower-sessions init, HKDF, watch channel lifecycle, `insert_build_log_row`, Stopping+ack-timer, shutdown ordering, `build_logs.finished` in recovery, all dep gaps — all confirmed fixed in the plan text.
- **8a/8b dispatch split.** Happy path and error paths independently testable. Exactly right.
- **Dispatch mutex + SQLite write ordering.** Held across DISPATCHED state write + `build_logs` row + watch sender creation, released before WS send. Explicitly documented.
- **REVOKING recovery sets `build_logs.finished = 1`.** Prevents SSE hang. Confirmed in Commit 12.
- **Watch sender lifecycle explicit.** Created at dispatch (Commit 8a), stored in `AppState.log_watchers`, dropped by `build_finished` handler and startup recovery. Clean separation from `ActiveBuild`.
- **sqlx bootstrap procedure.** Six numbered steps. Production-grade.
- **PASETO cross-language test.** Hardcoded expected bytes in Commit 3.
- **`pre_exec` safety.** "Only async-signal-safe functions — only `setsid()`." Correct.
- **Session key derivation.** HKDF-SHA256 with stable context string. Deterministic across restarts.
- **Seeding transaction ordering.** Commit transaction before printing keys. Orphan prevention.
- **Recovery before accepting connections.** Explicitly stated in Commit 12.
- **Phase 0 CLAUDE.md invariant list.** Dispatch mutex boundaries, pool sizing, trace_id lifecycle, watch sender ownership, tower-sessions init. Right mental model from the start.

---

## Open Questions (non-blocking)

1. **`trace_id` on re-dispatch:** update DB to new UUID, or retain first dispatch's UUID?
2. **`DELETE /api/auth/api-keys/{prefix}` prefix collision:** how does admin deletion scope to the correct owner?
3. **Components scan:** startup cache or per-request?
4. **`glob-match` `*` semantics:** resolve before Commit 5.
5. **Worker container Python environment:** Python version, cbscore install mechanism, config injection. Operational blocker for end-to-end Commit 11 testing.
