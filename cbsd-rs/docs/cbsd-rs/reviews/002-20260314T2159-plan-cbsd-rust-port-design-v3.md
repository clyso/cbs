# Plan Review: cbsd-rs Implementation Plans v3 (Phases 0–6)

**Plans reviewed:**


- `cbsd-rs/docs/cbsd-rs/plans/README.md` through `002-20260318T1411-05-integration.md` (all 9 files)


**Cross-referenced against:**

- `cbsd-rs/docs/cbsd-rs/design/` (all 4 design documents)

---

## Summary

The plans have been refined to a high level of quality over three review passes. Prior blockers (missing `grace_period_secs`, `descriptor_version`, `BuildQueue` split ambiguity, Commit 8 scoping) are all resolved. The commit ordering is correct, the dependency graph is sound, the 8a/8b dispatch split is clean, and the `trace_id` story is coherent end-to-end.

Two blockers remain, both documentation gaps. B1: the `tower-sessions-sqlx-store` initialization step (`.migrate().await`) is absent from Commit 2's server lifespan — without it, the first OAuth login attempt produces a runtime SQL error on the nonexistent `tower_sessions` table while health checks pass. B2: the `hkdf` crate required for session key derivation is absent from the project structure dependencies. Five significant concerns address lifecycle gaps (watch channel storage, `insert_build_log_row` attribution, Stopping+ack-timer interaction, shutdown log-drain ordering, `build_logs.finished` in startup recovery).

**Verdict: Approve with conditions.** Fix B1–B2. Address SC1–SC5 before the relevant commits. Phases 0 and 1 can proceed immediately.

---

## Blockers

### B1 — `tower-sessions-sqlx-store` initialization absent from Commit 2 lifespan

Commit 2's server lifespan: "open DB pool → run migrations → build router → serve." Missing: construct `SqliteStore`, call `.migrate().await` to create the `tower_sessions` table, wire into router via `SessionManagerLayer`. Without this, the table doesn't exist. Health checks pass. First OAuth login attempt hits a SQL error on `tower_sessions`.

The design doc correctly notes this table "is managed by the library, not cbsd migrations" and lists it as a startup step. The plan omits it.

**Fix:** Add to Commit 2's lifespan, after "run migrations": "Construct `SqliteStore` from the pool, call `.migrate().await` to create the `tower_sessions` table, wire into router via `CookieManagerLayer` + `SessionManagerLayer`." Also add to Phase 0's CLAUDE.md invariant list.

### B2 — `hkdf` crate missing from project structure dependencies

Commit 4 specifies session key derivation via HKDF-SHA256 with context `cbsd-oauth-session-v1`. This requires `hkdf = "0.12"` in `cbsd-server/Cargo.toml`. The project structure document lists dependencies exhaustively but does not include it.

If the implementer substitutes a simpler approach (raw key reuse, random key), sessions break across restarts or domain separation is violated.

**Fix:** Add `hkdf = "0.12"` to the project structure's `cbsd-server` dependency block. Add to Commit 4: "Derive session key: `Hkdf::<Sha256>::new(None, key).expand(b\"cbsd-oauth-session-v1\", &mut out)`."

---

## Significant Concerns

### SC1 — Watch channel storage and lifecycle unspecified across Commits 8a and 9


The log writer (Commit 9) creates `tokio::sync::watch` senders per active build. The SSE handler needs to find them. The plan never specifies:

1. Where the `HashMap<BuildId, watch::Sender<()>>` lives (in `ActiveBuild`? In `AppState` separately?).
2. Who creates the sender — dispatch (Commit 8a) or first `build_output` (Commit 9)? If at first output, there's a race where SSE connects before output arrives.
3. Who drops the sender on completion — `build_finished` handler, startup recovery (Commit 12), or shutdown (Commit 13)?

Storing in `ActiveBuild` couples the queue struct to the log subsystem. Storing separately in `AppState` is cleaner.

**Fix:** Add to Commit 9 (or CLAUDE.md): "Watch senders stored as `HashMap<BuildId, watch::Sender<()>>` in `AppState`, separate from `ActiveBuild`. Created at dispatch time (Commit 8a writes both `build_logs` row and watch sender). Dropped by `build_finished` handler and by startup recovery."

### SC2 — `insert_build_log_row()` function unattributed

Commit 8a's dispatch path specifies: "insert `build_logs` row under mutex." But `db/builds.rs` is defined in Commit 6 with functions `insert_build`, `update_build_state`, `get_build`, `list_builds` — no `insert_build_log_row()`. The function must exist before Commit 8a can write the dispatch transaction.

**Fix:** Add `insert_build_log_row(build_id, log_path)` to Commit 6's `db/builds.rs` function list. Or add it to Commit 8a with an explicit `cargo sqlx prepare` re-run note.

### SC3 — Stopping-state + ack-timer interaction has no test case

The design specifies: "If `worker_stopping` arrives mid-dispatch, the ack timeout must check for `Stopping` state and not mark the worker as 'suspect'." Neither Commit 7 nor Commit 8b lists this as a test case. If the ack timeout handler doesn't branch on worker state, the suspect-marking design invariant is silently missing.

**Fix:** Add to Commit 8b's testable items: "`worker_stopping` received mid-dispatch, ack timeout fires → build re-queued, worker state remains `Stopping` (not suspect)." Note in implementation bullets that the ack timeout handler must check `WorkerState::Stopping`.

### SC4 — Shutdown log-drain ordering loses final output lines

Commit 13's drain sequence: (5a) close WS connections → (5b) drain log writer → (5c) flush metadata. Closing WS before draining means any `build_output` messages buffered in the WS receive queue but not yet processed are silently lost.

**Fix:** Reverse: drain the log writer (receive all remaining WS messages during the drain timeout window) → close WS connections → flush metadata. The drain timeout (step 3) is the window for receiving final output; WS close must happen after, not before.

### SC5 — Startup recovery does not set `build_logs.finished = 1` for REVOKING → REVOKED

Commit 12 marks REVOKING builds as REVOKED but doesn't mention setting `build_logs.finished = 1`. A REVOKED build with `finished = 0` causes the SSE handler to hang waiting for a `done` event that never arrives (the watch sender was dropped on recovery, but the SSE client polls `finished` in the DB).

**Fix:** Add to Commit 12: "For REVOKING → REVOKED transitions, also set `build_logs.finished = 1`."

---

## Minor Issues

- **`uuid`, `clap`, `tokio-util` missing from project structure deps.** All three were flagged in prior reviews and remain absent. `uuid` (Commits 7/8a), `clap` (Commits 2/13), `tokio-util` (Commit 7).
- **`cargo sqlx prepare` re-run not reinforced after Commit 2.** Commits 6, 8a, 9 all add new `sqlx::query!` macros. Add a one-liner to CLAUDE.md: "After any commit that adds sqlx queries, re-run `cargo sqlx prepare --workspace` and include `.sqlx/` in the commit."
- **`cbscore-wrapper.py` not listed as a Commit 11 deliverable.** The script is referenced as pre-existing but doesn't exist. Add "Create `cbsd-rs/scripts/cbscore-wrapper.py`" to Commit 11.
- **`component.rs` listed in project structure but not in Commit 1.** Either add to Commit 1 or note it's added later (Commit 6 or 9).
- **`GET /builds/{id}/logs/tail` for QUEUED builds undefined.** No `build_logs` row exists. Specify: return 404 with `{"detail": "no logs yet"}`.
- **Worker backoff ceiling clamping vs. server panic: asymmetry undocumented.** Add one sentence to Commit 10: "The worker clamps silently (self-corrects); the server panics (operator error). This asymmetry is intentional."
- **`trace_id` column nullability.** `builds.trace_id TEXT` is NULL for QUEUED builds (set at dispatch). Note the nullability as intentional in Commit 2's schema description.
- **Commit 10 testable claim requires Phase 4 server.** Qualify: "Reconnects after server restart (integration test requires Phase 4 complete)."
- **`build_logs` absence for QUEUED builds — no recovery note.** Add to Commit 12: "QUEUED builds have no `build_logs` entry; one is created when dispatched in Commit 8a."
- **GC `JoinHandle` should be stored in `AppState`.** The 30-second sweep (Commit 8b) and the daily GC (Commit 13) — both background tasks — need tracked handles for clean shutdown.

---

## Suggestions

- **Add `tower-sessions-sqlx-store` init to CLAUDE.md invariant list** alongside the 6 existing invariants.
- **Embed the reconnection decision table as a Rust comment** above the `worker_status` handler in `ws/handler.rs` (Commit 8b).
- **`serde(skip_serializing_if = "Option::is_none")`** for `build_finished.error` in proto `ws.rs`. The design shows `"error": null` — note whether the intent is explicit null or absent field.
- **Re-dispatch sweep `JoinHandle` tracking:** Verify whether the 30-second sweep is a detached spawn or part of a `tokio::select!` loop. If background task, store handle in `AppState`.
- **Name `app.rs` as canonical `AppState` location** in Commit 2, noting subsequent commits extend the struct.

---

## Strengths

- **8a/8b dispatch split.** Happy path and error paths independently testable at ~400 lines each. Exactly right.
- **`active`/`workers` ownership boundary explicit.** Commit 6 owns queue lanes; Commit 7 adds `active` + `workers`. Clean, documented.
- **`trace_id` fully traceable.** Phase 0 CLAUDE.md → Commit 1 proto → Commit 2 schema → Commit 8a generation under mutex → Commit 11 subprocess propagation. End-to-end.
- **`pre_exec` safety note.** "Only async-signal-safe functions — no logging, no allocations, only `setsid()`." Correct and critical.
- **Shutdown sequencing precise.** SIGTERM = no revoke (workers reconnect). SIGQUIT = revoke + drain. The asymmetry is clearly motivated.
- **sqlx bootstrap procedure.** Six numbered steps in Commit 2. Production-grade.
- **PASETO cross-language test.** Hardcoded expected bytes, not emergent ordering. Directly addresses the canonical form requirement.
- **Design–plan authority chain.** "Design docs win over plans" — correct posture, unambiguously stated.
- **First-startup seeding.** Transaction commits before keys printed. Orphan prevention.

---

## Open Questions

1. **Watch sender storage location?** `ActiveBuild` (coupling) or separate `HashMap` in `AppState` (clean)? Must decide before Commit 8a.
2. **`component.rs` in Commit 1 or later?** Project structure lists it; Commit 1 doesn't.
3. **`GET /builds/{id}/logs` for QUEUED builds?** 404 with "no logs yet"? Specify in Commit 6 or 9.
4. **Worker silent clamping vs. server panic — is the asymmetry intentional?** Confirm in Commit 10.
5. **`build_logs.finished = 1` on REVOKING → REVOKED in recovery?** Must be set or SSE hangs.
6. **Python wrapper script — which commit creates it?** 0, 10, or 11?
