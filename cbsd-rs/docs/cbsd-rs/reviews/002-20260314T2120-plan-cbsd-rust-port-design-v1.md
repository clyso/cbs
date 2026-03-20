# Plan Review: cbsd-rs Implementation Plans (Phases 0–6)

**Plans reviewed:**
- `_docs/cbsd-rs/plans/README.md` (index, dependency graph, conventions)
- `_docs/cbsd-rs/plans/CLAUDE.md` (implementation session instructions)
- `_docs/cbsd-rs/plans/phase-0-scaffolding.md` through `phase-6-integration.md`

**Cross-referenced against:**
- `_docs/cbsd-rs/design/README.md`
- `_docs/cbsd-rs/design/2026-03-13-cbsd-rust-port-design.md`
- `_docs/cbsd-rs/design/2026-03-13-cbsd-auth-permissions-design.md`
- `_docs/cbsd-rs/design/2026-03-14-cbsd-project-structure.md`

---

## Summary

The plans are well-organized with sound commit ordering, clean phase boundaries, and honest testable outcomes. The 14-commit structure across 7 phases maps correctly onto the design's subsystem boundaries. The CLAUDE.md scaffolding makes implementation context self-contained, and the "design docs are authoritative; if plan and design disagree, design wins" posture is exactly right.

Three blockers must be resolved before Commit 1 is written. The most significant is B1: the `welcome` message is missing `grace_period_secs` — the design says the worker validates its backoff ceiling against this value, but neither the design's `welcome` schema nor the plan's `ws.rs` struct includes the field. B2 (`descriptor_version` column omitted from Commit 2's schema spec) and B3 (`BuildQueue` struct incompletely defined in Phase 3, creating a non-compiling gap at Phase 4) round out the blockers.

**Verdict: Approve with conditions.** Fix B1–B3, address the major concerns in-place, and proceed.

---

## Blockers

Issues that must be resolved before the relevant commit is written.

### B1 — `welcome` message missing `grace_period_secs`

The design states: "The worker validates its own backoff ceiling against the grace period value received in the `welcome` message." But the `welcome` JSON schema has only `protocol_version` and `connection_id`. Phase 4 Commit 7 reproduces this definition, and Phase 5 Commit 10 says the worker validates its ceiling "against grace period from `welcome` message" — with no field to read.

**Fix:** Add `grace_period_secs: u64` to the `Welcome` struct in `cbsd-proto/ws.rs` (Commit 1). Annotate in Commit 7 (server sends it) and Commit 10 (worker reads and validates it). Also update the design doc's `welcome` schema.

### B2 — `descriptor_version` column absent from Commit 2 schema spec

The design specifies `builds.descriptor_version INTEGER NOT NULL DEFAULT 1` with explicit semantics (unknown versions cause deserialization error, Python rows receive DEFAULT 1). Commit 2 lists "All 8 tables" and indexes but doesn't mention this column. An implementer writing the migration from this plan will omit it.

**Fix:** Add an explicit bullet to Commit 2: "include `descriptor_version INTEGER NOT NULL DEFAULT 1` in `builds` table." Add to testable items: "persisted build records have descriptor_version=1."

### B3 — `BuildQueue` struct incompletely defined in Phase 3

Phase 3 Commit 6 defines `BuildQueue` with only 3 `VecDeque` priority lanes. The design's authoritative struct includes `active: HashMap<BuildId, ActiveBuild>` and `workers: HashMap<ConnectionId, WorkerState>`. Phase 4 Commit 7 creates worker tracking in `ws/handler.rs` separately. Phase 4 Commit 8 then needs both structures for dispatch.

This creates an ambiguity: who owns the `active` map? Is it in `BuildQueue` (as the design specifies) or in the WS handler? An implementer following Phase 3 literally will produce a struct that Phase 4 must retroactively amend.

**Fix:** Either (a) define the full `BuildQueue` struct in Commit 6 with `active` and placeholder `workers` fields (even though dispatch populates them in Phase 4), or (b) split the concern explicitly: Phase 3 owns the queue lanes; Phase 4 Commit 7 extends the struct with `active` and `workers`. Either way, make the ownership boundary explicit so the struct definition compiles at every commit boundary.

---

## Major Concerns

Significant issues that will cause pain if not addressed.

### M1 — Commit 8 is too large — ~600–900 lines of async state machine code

Commit 8 covers: split-mutex dispatch, DISPATCHED state DB write, ack timeout lifecycle, `build_accepted`/`started` transitions, `build_rejected` handling (including integrity failure → FAILURE), `DELETE /builds/{id}` extension for DISPATCHED/STARTED/REVOKING, revoke ack timeout, the full 10-row reconnection decision table, grace period expiry transitions, and the 30-second periodic sweep. This touches at least 4 files.

If Commit 8 fails to compile or has a logic error, there's no intermediate checkpoint between "basic hello/welcome" (Commit 7) and the full dispatch engine. Debugging a 900-line async state machine commit is significantly harder than debugging two 400-line commits.

**Fix:** Split into Commit 8a (split-mutex dispatch: enqueue → dispatch → build_accepted → build_started → build_finished — the happy path) and Commit 8b (revocation flow, reconnection decision table, grace period expiry, periodic sweep). Each is independently testable.

### M2 — `build_revoke`-before-`build_accepted` handling in wrong commit

Commit 7 specifies this protocol rule in `ws/liveness.rs`, but `build_revoke` is only sent by the dispatch engine (Commit 8). There's no production code path in Commit 7 that triggers it, so it can't be tested there.

**Fix:** Move the pre-accept revoke handling to Commit 8 (or 8b if split) where it can actually be tested as part of the dispatch flow.

### M3 — 30-second periodic sweep misattributed between phases

Phase 4 Commit 8 implements the sweep (`tokio::time::interval`). Phase 6 Commit 13 claims it in its testable items: "30-second re-dispatch sweep catches orphaned queued builds." But the sweep is already running by Commit 8.

**Fix:** Add to Commit 8's testable items: "30-second periodic sweep starts and dispatches queued builds." Remove from Commit 13.

### M4 — HKDF session key derivation absent from Phase 2

The design specifies session signing key = HKDF-SHA256 of `token_secret_key` with context `cbsd-oauth-session-v1`. Phase 2 Commit 4 specifies the `tower-sessions` SQLite store but never mentions key derivation. Without HKDF, sessions won't survive server restarts (if random key) or domain separation is violated (if raw `token_secret_key` reused).

**Fix:** Add to Commit 4: "Derive session signing key from `token_secret_key` via HKDF-SHA256 with context `cbsd-oauth-session-v1`."

### M5 — `allow_any_google_account` startup guard absent

The design requires the server to refuse startup if `allowed_domains` is empty without explicit `allow_any_google_account: true`. No commit tracks this. A misconfigured deployment silently accepts any Google account.

**Fix:** Add to Commit 2 (config validation) or Commit 4 (OAuth setup): "Panic at startup if `oauth.allowed_domains` is empty and `oauth.allow_any_google_account` is not `true`."

### M6 — `build_logs` INSERT timing unclear in Commit 9

Commit 9 specifies `build_logs.finished = 1` set on `build_finished` and `log_size` updated periodically. But the initial `build_logs` row INSERT (creating the record for a new build's log) is not specified in any commit. It should happen at dispatch time (Commit 8, when the build transitions to DISPATCHED) or at first `build_output` arrival (Commit 9). The plan leaves this to the implementer's judgment, but the SSE handler (also Commit 9) queries `build_logs` — if the row doesn't exist yet, it gets a 404 for a valid in-progress build.

**Fix:** Add to Commit 8: "Insert `build_logs` row at dispatch time (DISPATCHED state) with `log_path`, `log_size=0`, `finished=0`."

---

## Minor Issues

- **`trace_id` generation site unspecified.** Commit 8 sends `build_new` with `trace_id` but doesn't say when/how it's generated. Add: "Generate `trace_id` (UUID v4) at dispatch time."
- **PASETO cross-language CI test missing.** The design requires Python/Rust SHA-256 equality assertion. This belongs in Commit 3's testable items.
- **`descriptor_version` unknown-version error path not tracked.** The design says unrecognized versions cause a deserialization error. No commit mentions implementing or testing this guard.
- **`cbsd api-keys create --db` CLI mode absent.** Design mentions it; plans don't. Either add to Commit 12 or defer explicitly in the README's deferred section.
- **`.sqlx/` offline cache not in any commit's completion criteria.** Commit 2 introduces the first sqlx queries. Add: "Run `cargo sqlx prepare`, commit `.sqlx/` directory."
- **`ActiveBuild` struct undefined.** The design uses `ActiveBuild` in the dispatch critical section. The plan never defines it. Needs a home (Commit 6 or Commit 8) and a definition (at minimum: `build_id`, `connection_id`, `dispatched_at`).
- **`ws/dispatch.rs` ↔ `ws/handler.rs` call direction unclear.** Commit 7 creates the handler; Commit 8 creates dispatch. Does handler call into dispatch, or vice versa? Specify to avoid circular module deps.
- **`--drain` CLI flag not mentioned in Commit 13.** Design says `SIGQUIT or --drain`. The flag requires a `clap` argument. One-liner but should be tracked.
- **Phase 6 Commit 12 seeding: plaintext printed before transaction commits.** If the transaction rolls back after printing worker API keys, the printed plaintext has no DB record. Specify: generate all keys → commit transaction → print plaintext only on success.
- **`cbs.component.yaml` filename is correct.** Verified against actual `components/` directory. The open question in the design README can be closed.
- **Commit 10 parallelism note is slightly misleading.** Config/connection/signal (Commit 10) can parallel Phase 4. But `ws/handler.rs` message dispatching requires Phase 4 server-side behaviors. Clarify: Commit 10 infra parallels Phase 4; Commit 11 requires Phase 4 complete.

---

## Suggestions

- **Split Commit 8** into 8a (happy-path dispatch) and 8b (revocation + reconnection). Keeps each under ~400 lines with independent testability.
- **Phase 5 Commit 10 can start before Phase 4.** Write worker config, WS connection, reconnect backoff against proto types with a stub server. Worth exploiting for parallel development.
- **Add `(state, queued_at)` composite index** to Commit 2's migration. The startup recovery query (Phase 6) scans `queued` builds ordered by `queued_at`. Cheap now, expensive to add later.
- **Add `UNIQUE(name, owner_email)` constraint on `api_keys`** to Commit 2's migration. Prevents confusing duplicate-named keys per user.
- **Consider a `build_logs(finished, updated_at)` composite index** in Commit 2 for the GC query (Phase 6).

---

## Strengths

- **Sound commit ordering.** Every commit depends only on artifacts from prior commits. The dependency graph is correct and explicit.
- **`cbsd-proto` as first-class foundation (Commit 1).** All shared types with zero IO deps, serde round-trip tests from the start. Both server and worker have compile-time wire-format agreement immediately.
- **Testable outcomes are concrete and honest.** "Server boots, runs migrations, responds to health check" (Commit 2). "End-to-end: submit → dispatch → stream → complete" (Commit 11). These are genuine checkpoints.
- **Phase 6 handles operational cases as first-class work.** Startup recovery and dual shutdown modes are not afterthoughts.
- **Split-mutex dispatch correctly scoped in Commit 8.** The hardest correctness property in the design is captured with full detail.
- **CLAUDE.md plan-level document is well-structured.** "Design docs are authoritative; if plan and design disagree, design wins" — correct posture.
- **Phase 3 correctly defers DISPATCHED/STARTED revocation to Phase 4.** Clean commit boundary.
- **Commit 11 faithfully reproduces all 5 exit code classifications.** No hand-waving on subprocess bridge.

---

## Open Questions

- **Does `welcome` carry `grace_period_secs`?** Design prose says yes; schema says no. Must be resolved before Commit 1 freezes `ws.rs`.
- **Which commit owns `active` and `workers` in `BuildQueue`?** Determines whether Phase 3 or Phase 4 defines the complete struct.
- **Is `cbsd api-keys create --db` CLI mode in scope for v1?** If yes, add to Commit 12. If no, add to README deferred section.
- **What is `ActiveBuild`'s struct definition and which commit defines it?** At minimum: `build_id`, `connection_id`, `dispatched_at`.
- **Call direction between `ws/handler.rs` and `ws/dispatch.rs`?** Handler dispatches to dispatch module, or dispatch holds channel to handler?
- **Phase 6 Commit 12: print API keys before or after transaction commit?** After-only prevents orphaned printed keys on rollback.
