# Plan — Security Audit Remediation Implementation (Unified)

| Field         | Value                                                                                                                                                                                                                   |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Plan          | 019 — security-audit-remediation, v2 (unified)                                                                                                                                                                          |
| Designs       | `cbsd-rs/docs/cbsd-rs/design/019-20260426T1154-worker-control-plane-hardening.md` (WCP, Draft v11, authoritative) + `cbsd-rs/docs/cbsd-rs/design/019-20260514T1040-security-audit-remediation.md` (audit-rem, Draft v8) |
| Date          | 2026-05-16                                                                                                                                                                                                              |
| Status        | Draft v2                                                                                                                                                                                                                |
| Supersedes    | `cbsd-rs/docs/cbsd-rs/plans/019-20260516T0758-security-audit-remediation.md` (v1; covered only audit-remediation commits + gated Phase E)                                                                               |
| Scope         | Unified commit breakdown covering both authoritative designs at seq 019. 21 commits total: 6 WCP-foundational + 12 audit-remediation cross-cutting + 3 WCP-extension (audit-remediation D11/D12/D13).                   |
| Soundness ref | `cbsd-rs/docs/cbsd-rs/reviews/019-20260516T0952-design-wcp-soundness-v1.md` — confirms WCP and audit-remediation v8 do not conflict and identifies the 10 implementation gaps (G1–G10) that this plan absorbs.          |

## Overview

This plan supersedes the v1 plan (which deferred WCP work as "Phase E blocked on
WCP plan"). Per the soundness review, **WCP v11 is sound and authoritative**;
the WCP design's gap is purely between spec and code — no design changes are
needed. The plan absorbs WCP's foundational work (G1-G10 from the soundness
review) as concrete commits, alongside the audit-remediation cross-cutting
commits and the three audit-remediation WCP-extension commits.

**Commit-message format** follows `git-commits`: `component: short description`
with a 2-4 sentence body where needed. Component convention per the project's
existing git log: `cbsd-rs/<crate-suffix>:` for crate-scoped work (e.g.,
`cbsd-rs/server`, `cbsd-rs/worker`, `cbsd-rs/proto`, `cbsd-rs/cbc`); plain
`cbsd-rs:` for workspace-spanning commits.

**Code samples are not included** in this plan — they live in the authoritative
design documents where they exist. The plan focuses on capability delivered,
ordering, dependencies, LOC budget, and pitfalls that affect implementation
correctness.

### Three logical phases (with internal ordering only)

The plan organises commits into three logical phases. Phases are not strict
barriers — within a phase, commits may interleave freely unless an explicit
dependency is noted.

- **Phase 1 — WCP foundational** (6 commits): implements WCP D1-D7's worker
  control plane policy. Closes the four findings from the original security
  review (cross-worker lifecycle spoof, log spoof, empty components, full-file
  log tail) and establishes the authorization, rollback, and supervisor
  primitives that Phase 3 builds on.
- **Phase 2 — Audit-remediation cross-cutting** (12 commits): implements
  audit-remediation D1-D10. Independent of Phase 1's worker control plane work.
  Closes audit findings F1, F2, F4, F5, F7, F8, F10, F11, F13.
- **Phase 3 — WCP-extension** (3 commits): implements audit-remediation D11,
  D12, D13. Requires Phase 1's WCP supervisor model, dispatch rollback function,
  and ownership checks to exist.

### Ordering constraints

The only hard ordering constraints are:

**Within Phase 1**:

- W1 (rollback DB operation) before W2 (build-scoped authorization). W2's
  transient-reject path uses W1's rollback function.
- W2 before W3 (DB-backed reconnect migration). W3 extends W2's ownership model
  to the reconnect/idle paths.

**Within Phase 2**:

- AR7 (URI logging redaction) before AR8 (`Secret<T>` wrap). AR8 builds on AR7's
  logging policy.
- AR8 before AR9 (`tracing` audit). AR9 audits call sites against AR8's
  contract.

**Phase 3 depends on Phase 1**:

- E1 (accepted-phase reconnect) requires W6 (worker supervisor) and W4
  (`ActiveAssignmentReceipt` field, which is part of W2).
- E2 (liveness/dead-worker resolution) requires W1 (rollback fn) and W4 (receipt
  state).
- E3 (migration revoke + drain) requires W3 (DB-backed migration), W6
  (supervisor), and the protocol-extension support W2 establishes for the
  `UnauthorizedBuildAction` wire type.

Phase 2 (audit-remediation cross-cutting) has no _compile_ or _functional_
dependency on Phase 1 or Phase 3, but SHOULD land after Phase 1 is complete to
avoid a window in which message-level size bounds (commit 9) are in force
without the build-scoped ownership check (commit 2) that makes D6's trust
argument sound — see commit 9's pitfalls and the audit-rem design's note on the
Phase C / WCP ownership-rule interaction. The plan sequence (Phase 1 → Phase 2 →
Phase 3) already reflects this preference; interleaving is permitted only where
the security window remains closed.

## Commit breakdown

LOC numbers are estimates including tests; auto-generated artefacts (`.sqlx/`
cache, `Cargo.lock`) are excluded per the `git-commits` skill.

| #   | Phase | Component        | Subject                                                           | LOC  | Notes      |
| --- | ----- | ---------------- | ----------------------------------------------------------------- | ---- | ---------- |
| 1   | 1     | `cbsd-rs/server` | add rollback DB operation that clears stale assignment provenance | ~320 |            |
| 2   | 1     | `cbsd-rs`        | enforce build-scoped authorization on worker lifecycle messages   | ~720 |            |
| 3   | 1     | `cbsd-rs/server` | enforce DB-backed ownership on worker reconnect and idle status   | ~450 |            |
| 4   | 1     | `cbsd-rs/server` | reject empty `components` arrays at build submission              | ~180 | ⚠ small    |
| 5   | 1     | `cbsd-rs`        | bound log tail with reverse block scanning                        | ~330 |            |
| 6   | 1     | `cbsd-rs/worker` | introduce process-level supervisor for worker build state         | ~750 |            |
| 7   | 2     | `cbsd-rs`        | enforce strict `CBSD_DEV` parsing and loopback dev-mode           | ~360 |            |
| 8   | 2     | `cbsd-rs/worker` | enforce tarball containment and decompression cap                 | ~380 |            |
| 9   | 2     | `cbsd-rs`        | cap REST body and WebSocket message sizes                         | ~250 |            |
| 10  | 2     | `cbsd-rs/server` | reject OAuth callback when `email_verified` is false              | ~130 | ⚠ small    |
| 11  | 2     | `cbsd-rs/server` | split `periodic:manage` into `:own` and `:any` caps               | ~400 |            |
| 12  | 2     | `cbsd-rs/server` | re-validate periodic task scopes at trigger time                  | ~400 |            |
| 13  | 2     | `cbsd-rs`        | redact bearer tokens from URI logging surface                     | ~230 |            |
| 14  | 2     | `cbsd-rs`        | wrap token material in `Secret<T>`                                | ~650 |            |
| 15  | 2     | `cbsd-rs`        | redact token material from `tracing` call sites                   | ~200 | ⚠ at floor |
| 16  | 2     | `cbsd-rs/server` | index `api_keys.key_prefix` for O(log n) lookups                  | ~50  | ⚠ tiny     |
| 17  | 2     | `cbsd-rs/cbc`    | enforce HTTPS host and write config atomically                    | ~250 |            |
| 18  | 2     | `cbsd-rs/proto`  | add SI-18 regression test for `ServerMessage`                     | ~250 |            |
| 19  | 3     | `cbsd-rs/worker` | report `Building` during accepted-phase reconnect                 | ~250 |            |
| 20  | 3     | `cbsd-rs/server` | resolve dead workers by DB state and receipt                      | ~400 |            |
| 21  | 3     | `cbsd-rs`        | deliver migration revoke and drain terminal-pending               | ~700 |            |

Legend: ⚠ flagged below 200-LOC (justification under "Requested exceptions").
Total ~7450 LOC across 21 commits.

## Per-commit details

The per-commit sections below describe **capability delivered**, **packages
touched**, **notable pitfalls**, and **tests**. Code is not reproduced from the
designs.

### Phase 1 — WCP foundational

#### Commit 1 — `cbsd-rs/server: add rollback DB operation that clears stale assignment provenance`

**Closes** WCP D4 partial (the rollback column-clearing operation) + gap G3 +
dispatch ordering fix (gap G10).

**Capability**: a build that rolls back to `queued` after a dispatch failure or
transient reject no longer carries stale `worker_id`, `trace_id`, `error`,
`started_at`, `finished_at`, or `build_report` values from the abandoned
assignment. The reconnect Building handler ensures
`queue.active[].connection_id` is populated **before** `handle_build_started` is
invoked.

**Packages**: `cbsd-server` (`db/builds.rs` new function; `ws/dispatch.rs`
ack-timeout handler; `ws/handler.rs` reconnect Building handler ordering fix).

**Notable pitfalls**:

- The rollback function MUST clear all six columns (`worker_id`, `trace_id`,
  `error`, `started_at`, `finished_at`, `build_report`) in a single SQL update,
  not via the generic `update_build_state` helper. WCP D4 SI-6/SI-13 lists the
  exact column reset list — the dedicated operation owns this list and the
  generic helper does not.
- The dispatch ordering fix is **scoped to the reconnect path only**, not the
  normal-dispatch path. Verified against source:
  - Normal-dispatch flow: `ws/dispatch.rs:122-135` inserts the `ActiveBuild`
    into `queue.active` with `connection_id` already populated at insertion
    time. The subsequent `handle_build_started` call at `ws/handler.rs:519`
    (invoked from the `WorkerMessage::BuildStarted` dispatch arm) therefore sees
    a correctly-populated entry. **No ordering inversion on this path.**
  - Reconnect Building flow: `ws/handler.rs:661-665` invokes
    `dispatch::handle_build_started(state, build_id.0).await` **before**
    `queue.active.get_mut(&build_id.0).connection_id = …` runs. This is the
    inversion gap G10 refers to. The fix moves the `connection_id` assignment to
    land before the `handle_build_started` call (or merges them under a single
    queue-lock acquisition that updates the entry atomically).
- Existing `update_build_state("queued", ...)` callsites in `dispatch.rs`
  (ack-timeout) and any `handle_build_rejected` transient-reject path MUST
  migrate to the new rollback function in this same commit. Otherwise those
  paths still leak stale provenance.
- `cargo sqlx prepare --workspace` will need to be run after the new query is
  added; the `.sqlx/` cache delta is excluded from the LOC budget per the skill.

**Tests**: rollback-clears-all-columns unit test; dispatch-ack-timeout
integration test asserts the new rollback function is invoked; reconnect-path
ordering test asserts `connection_id` is populated before `handle_build_started`
runs on the Building reconnect handler at `ws/handler.rs:661-665`. No additional
test on the normal-dispatch path because `ws/dispatch.rs:122-135` is already
correctly ordered.

---

#### Commit 2 — `cbsd-rs: enforce build-scoped authorization on worker lifecycle messages`

**Closes** WCP D1 + D2 + D3 + D6 + gaps G1, G2, G9 (receipt-state introduction).

**Capability**: any worker that sends a `build_accepted`, `build_started`,
`build_finished`, `build_rejected`, or `build_output` message for a build that
is not currently assigned to its connection gets a non-fatal
`UnauthorizedBuildAction { build_id, action, reason: NotAssigned }` response and
(for execution-evidence messages — `build_started`, `build_output`) a
reporter-directed `BuildRevoke`. The original security review's F1 (cross-worker
lifecycle spoofing) and F2 (cross-worker log output spoofing) are closed by this
commit.

**Packages**: `cbsd-proto` (new `ServerMessage::UnauthorizedBuildAction`
variant, `WorkerBuildAction` enum, `UnauthorizedBuildReason` enum — wire types
only); `cbsd-server` (`queue/mod.rs` declares new `ActiveAssignmentReceipt` enum
alongside `ActiveBuild`, and `ActiveBuild` gains a
`receipt: ActiveAssignmentReceipt` field; `ws/handler.rs` per-handler ownership
check; `logs/writer.rs` ownership check before write); `cbsd-worker` (one-arm
WARN-and-continue handler for the new `ServerMessage::UnauthorizedBuildAction`
variant in `ws/handler.rs::match server_msg`, so the existing exhaustive match
still compiles).

**Notable pitfalls**:

- **Wire-protocol break risk on the worker side (golden-rule preservation).**
  Adding a new variant to `ServerMessage` in `cbsd-proto` is a compile break for
  any consumer that pattern-matches the enum exhaustively. Verified against
  source: `cbsd-worker/src/ws/handler.rs:186` opens
  `match server_msg { ServerMessage::BuildNew { … } => … }` with arms for every
  current variant and **no catch-all `_` arm**. Without an
  `UnauthorizedBuildAction` arm added in the same commit, `cbsd-worker` fails to
  compile, violating the `git-commits` golden rule (every commit must compile).
  This commit therefore MUST extend that match with a worker-side arm. The arm's
  behaviour at this commit is intentionally minimal — log a `tracing::warn!`
  with the build_id, action, and reason fields, and continue the connection
  loop. Active stop-work response (kill subprocess on stale-execution evidence)
  is part of commit 6 (worker supervisor) and intentionally NOT in this commit.
- The `UnauthorizedBuildAction` response is non-fatal — the worker connection
  MUST NOT be closed solely on this mismatch. WCP D2 is explicit about this. The
  worker-side arm added here does not close the connection.
- The worker-facing `reason` value is coarse (`NotAssigned` for every
  authorization failure today). Detailed `internal_reason` fields go to the
  server log only, never to the worker.
- The reporter-directed `BuildRevoke` for unauthorized `build_started` /
  `build_output` must NOT mutate the real assignment's DB state, timer, watcher,
  or `queue.active` entry. It is reporter-directed cleanup, not a state-mutating
  revoke.
- The `ActiveAssignmentReceipt` enum and the `receipt` field on `ActiveBuild`
  are introduced in `cbsd-server/src/queue/mod.rs` — **not** in `cbsd-proto`.
  Per WCP SI-25, receipt state lives in server process memory only: it is never
  serialized to the wire and is never reconstructed after server restart
  (startup recovery uses the existing fail-in-flight policy). `cbsd-proto` is
  chartered as wire-format only and is also depended on by `cbsd-worker` and
  `cbc`; placing a server-internal state type there would force unrelated
  consumers to compile a type they can never use and would imply a protocol
  surface that does not exist. The enum has two states (`AwaitingReceipt`,
  `ReceivedByWorker`). Initial value on dispatch insertion is `AwaitingReceipt`.
  `build_accepted` transitions it to `ReceivedByWorker`. Owned `build_started`
  and `build_output` from `dispatched` also transition it. This commit writes
  the receipt state; the first reader is commit 3 (idle reconnect rollback
  decision).
- The ownership-check helper should be centralised
  (`fn active_build_for_connection(&Queue, build_id, &str) -> Option<&ActiveBuild>`)
  to keep the four handlers consistent. WCP D1 explicitly calls for this
  centralisation.
- `write_build_output` (`logs/writer.rs`) is currently called from the handler
  without a `connection_id` parameter. The signature needs to grow that
  argument, threaded through `handler.rs:521-549`.
- Dispatch-ack timer cancellation per WCP D4: every valid owned message that
  proves receipt (including `build_accepted`, owned `build_started`, owned
  `build_output`, etc.) cancels the timer. The commit must update each handler
  to do this consistently.

**Tests**: cross-worker spoof rejection for each of the five lifecycle messages;
reporter-directed `BuildRevoke` for unauthorized `build_started` and
`build_output`; valid owned messages succeed and transition receipt state
correctly; coarse reason field is the only worker-facing reason; server log
records internal reason; **worker-side smoke test that
`ServerMessage::UnauthorizedBuildAction { build_id, action, reason }`
deserializes via `serde_json::from_str` and is consumed by the
`cbsd-worker/src/ws/handler.rs:186` match without panicking, producing the
expected `tracing::warn!` log line**.

---

#### Commit 3 — `cbsd-rs/server: enforce DB-backed ownership on worker reconnect and idle status`

**Closes** WCP D1 (reconnect ownership), WCP same-worker migration spec, WCP
idle reconnect spec, gaps G7, G8.

**Capability**: a worker reconnecting and reporting
`worker_status(Building { build_id })` no longer rewrites
`queue.active[build_id].connection_id` blindly. The server runs a two-phase
ownership check (snapshot under queue lock → DB-backed verification against
`builds.worker_id` → reacquire lock for the swap). A worker reporting
`worker_status(Idle)` cannot affect another authenticated worker's active
builds; idle reconciliation is scoped to the reporter's own persisted
assignments. F3 from the audit (reconnect ownership rewrite) is closed.

**Packages**: `cbsd-server` (`ws/handler.rs` reconnect handling; `ws/handler.rs`
idle reconcile; new helper for the two-phase ownership check that consults
`db::builds::get_build`).

**Notable pitfalls**:

- The two-phase check MUST NOT hold the queue lock across the DB query. WCP
  migration step 4-5 is explicit: snapshot candidate build IDs under the lock,
  release the lock, query the DB, then reacquire the lock for the swap. Holding
  the lock across SQLite I/O risks pool exhaustion + deadlock (per the cbsd-rs
  CLAUDE.md "Correctness Invariants" #2).
- Reconnect resume for `worker_status(Building)` validates against the persisted
  `builds.worker_id` matching the authenticated registered worker ID. Stale
  active entries whose DB row is no longer in `dispatched`/`started`/`revoking`
  MUST be removed (with log watcher) without DB mutation — WCP migration step 6
  handles this.
- Idle reconcile MUST filter by `builds.worker_id == authenticated`. The current
  code at `handler.rs:717-766` filters only on
  `ab.connection_id != connection_id` with worker state `Disconnected | Dead`,
  which leaks across workers (gap G8). The fix is the DB filter.
- Idle reconcile uses receipt state (added in commit 2): a
  `dispatched + AwaitingReceipt` entry rolls back to `queued` only when the
  previous same-worker connection was absent / disconnected / dead before this
  reconnect. A `dispatched + ReceivedByWorker` entry rolls back even when the
  previous connection was live. WCP idle-reconnect table is the authoritative
  spec.
- The reporter-directed `BuildRevoke` from commit 2 is reused for invalid
  `worker_status(Building)` claims. The handler must also emit
  `UnauthorizedBuildAction`.

**Tests**: cross-worker `worker_status(Building)` is rejected with
`UnauthorizedBuildAction` + reporter-directed `BuildRevoke` and does not move
active ownership; same-worker migration with valid persisted ownership succeeds;
cross-worker idle status does not mutate other workers' builds; same-worker idle
reconcile uses receipt state correctly for the two `dispatched` sub-states.

---

#### Commit 4 — `cbsd-rs/server: reject empty components arrays at build submission`

**Closes** WCP D5, gap G4. Closes the prior-review finding F3 ("empty component
lists can strand dispatch").

**Capability**: `submit_build`, periodic-task create/update, and the scheduler
trigger reject build descriptors whose `components` array is empty. Invalid
stored periodic descriptors are fatal at trigger time: the task is disabled with
`last_error` recording the validation failure, no build is enqueued, no
retry/backoff.

**Packages**: `cbsd-server` (`routes/builds.rs`, `routes/periodic.rs`,
`scheduler/trigger.rs`, plus a centralised typed descriptor validator per WCP D5
— likely a new `components/validator.rs` module).

**Notable pitfalls**:

- WCP D5 calls for a **single shared validator** used by all ingress paths (REST
  submit, periodic create, periodic update, scheduler trigger). Writing
  per-route ad-hoc checks is the anti-pattern.
- The validator's responsibility is broader than empty-components: every listed
  component name must be known to the server's component registry; repository
  scope checks remain tied to the typed component list. The validator's input is
  the parsed `BuildDescriptor`; output is a `Result<(), ValidationError>` with
  enough detail for the trigger's `last_error` field.
- Scheduler trigger paths that fail validation MUST disable the periodic task
  and persist the validation message — not retry. Re-enabling requires a valid
  update through the normal periodic update path.
- The dispatch path keeps its empty-component invariant guard (an internal
  sanity check that rolls back to `queued` if encountered via legacy/corrupted
  data). This catches the case where a row was written before commit 4 and is
  read after.

**Tests**: REST submit with empty `components` → 400; periodic create/update
with empty `components` → 400; periodic trigger fires on a stored invalid
descriptor (legacy) → task disabled, `last_error` set, no build enqueued,
scheduler continues with other tasks.

⚠ Size: ~180 LOC, below the 200-LOC floor. Justified — the validator module is
small (one main function + variants for each ingress path), and combining with
adjacent commits would mix concerns.

---

#### Commit 5 — `cbsd-rs: bound log tail with reverse block scanning`

**Closes** WCP D7, gap G5. Closes the prior-review finding F4 ("log tail reads
full file into memory").

**Capability**: `GET /api/builds/{id}/logs/tail` reads only the newest ~1000
lines (or whatever the request asks for, capped at 1000) via reverse block
scanning bounded by `MAX_TAIL_BYTES = 4 MiB`. Memory use is independent of total
log size. The response shape changes: the inexact `total_lines` field is
removed; new fields `returned`, `requested`, `truncated`, `bytes_scanned`, and
`max_tail_bytes` are added. `cbc logs tail` defaults to `n=50`.

**Packages**: `cbsd-server` (`routes/builds.rs::logs_tail` rewrite, new helper
for reverse block scanning), `cbsd-rs/cbc` (default arg change + handle new
response shape).

**Notable pitfalls**:

- `MAX_TAIL_LINES` is reduced from 10000 (current) to 1000.
- Scanning begins from EOF; if the file's last byte is mid-line, the partial
  trailing line is dropped (return only complete lines).
- UTF-8 boundary safety: if the retained byte window starts mid-code- point,
  drop the leading partial code-point. Return only valid UTF-8.
- If a single line exceeds the byte budget, the endpoint cannot return a
  complete line within budget. It returns no partial line, sets
  `truncated: true`, and surfaces a warning/detail field. WCP D7 specifies this
  behavior.
- `cbc` must NOT require the old `total_lines` field in its deserialization. It
  renders `returned`, `requested`, `truncated` instead. The client default
  request count changes from 30 to 50.
- The full-log streaming endpoint is unchanged; this commit affects only the
  JSON tail endpoint.

**Tests**: 4 MiB log file tailed without OOM; UTF-8 boundary drops partial
code-point; partial trailing line dropped; single line over budget →
`truncated: true`, no partial line returned; `cbc` round- trips the new response
shape without expecting `total_lines`.

---

#### Commit 6 — `cbsd-rs/worker: introduce process-level supervisor for worker build state`

**Closes** WCP "Worker-Side Active Build State" section, gap G6.

**Capability**: the worker's active-build state lives in a process-level
supervisor that outlives any individual websocket connection. The supervisor
tracks at minimum: build ID, local execution phase (`accepted` / `started` /
`revoking` / `terminal-pending-report`), executor handle, component working
directory, pending terminal result, and a bounded local output spool. Reconnect
status is derived from the supervisor: `Building` if the supervisor has any
non-terminal local assignment state, `Idle` only when nothing is pending.

**Packages**: `cbsd-worker` (new supervisor module replacing the local-variable
active-build state; `ws/connection.rs` becomes a transport client that forwards
to/from the supervisor; build output path uses the new spool).

**Notable pitfalls**:

- The supervisor is the largest single change in the plan. There is no clean
  smaller-commit split: the websocket loop currently owns the active build
  state, and decoupling them requires landing the supervisor + transport
  refactor in one go.
- A websocket receive/send error does NOT by itself kill the build. The
  supervisor keeps the subprocess and local assignment state until it receives a
  `BuildRevoke`, the process exits, or local worker shutdown stops it. The
  current code's tendency to lose state on any disconnect is what gap G6
  addresses.
- Output spool budget: 64 MiB per active build. Spool file is per-build under
  the worker's temp dir. Overflow kills + awaits the subprocess and records the
  failure reason; the worker MUST NOT continue silently while dropping output.
- `terminal-pending-report` semantics: when the subprocess completes during a
  disconnect, the supervisor stores the terminal result and reports
  `WorkerStatus(building)` plus the pending result on the next reconnect, before
  sending the `build_finished` payload. Order matters — Building first, then
  send pending output, then send terminal `build_finished`.
- On reconnect, if the supervisor cannot determine whether an active subprocess
  still exists (e.g., the process is being torn down), it MUST stop and await
  any possible child before reporting anything. Reporting Idle while a child is
  still running is the bug class G6 exists to close.
- This commit lands without `last_authenticated_connect_at` (D13's anti-coercion
  clock). That field is added in commit 21.

**Tests**: subprocess survives a websocket drop and is reported on reconnect;
spool overflow kills the build and emits a failure report;
`terminal-pending-report` survives reconnect and is delivered after the Building
announcement; supervisor's per-phase state transitions match the WCP spec;
supervisor reports Idle only when no executor/revoke/pending-result/accepted
state exists (the commit 19/E1 follow-up extends this with the accepted-phase
rule).

---

### Phase 2 — Audit-remediation cross-cutting

#### Phase 1 carry-over — `try_dispatch` send-failure end-to-end test

**Origin:** Phase 1 review v4
(`cbsd-rs/docs/cbsd-rs/reviews/019-20260519T0857-impl-security-audit-remediation-phase-1-v4.md`),
finding NB1. Marked non-blocking (Nit, 0 points deducted) by the v4 reviewer and
explicitly deferred to Phase 2 by user decision after a scope-budget check.

**Gap.** `cbsd-server/src/ws/dispatch.rs::send_and_recover` (extracted in Phase
1 commit 1) has unit-level regression coverage for the rollback-on-send-failure
invariant: its tests assert that a closed receiver and a missing sender both
trigger the full rollback. No test drives `try_dispatch` itself end-to-end with
an injected broken sender, so a maintainer reverting the single
`send_and_recover(...).await?` call site at `try_dispatch` step 11–12 to inline
send + buggy rollback would not fail any test.

**Why not landed in Phase 1.** Closing NB1 requires introducing `AppState` test
scaffolding, which Phase 1 explicitly avoided per the lightweight-extraction
policy. The reviewer marked NB1 with zero deduction; the user opted to defer
rather than reverse the policy mid-phase.

**Recommendation.** Land as the first commit of Phase 2, or fold into commit 7
if the test-scaffolding work naturally co-locates with that commit's
`cbsd-common` introduction. Sketch:

1. Add a `#[cfg(test)] fn test_app_state(...)` factory that builds a minimal
   `AppState` using `OAuthState::dummy()`, `TokenCache::new(64)`,
   `TimeoutsConfig::default()`, and default sub- configs for
   `LogRetentionConfig`, `SeedConfig`, `DevConfig`, `LoggingConfig`. The
   `secrets` and `oauth` sub-configs are constructed inline. ~30 LOC.
2. Add a `temp_component_dir()` helper that writes a minimal
   `cbs.component.yaml` so the tarball-pack step succeeds. ~10 LOC.
3. Add a test `try_dispatch_send_failure_rolls_back_db_end_to_end` that: builds
   the test `AppState` pointing at the temp component dir; inserts a queued
   build + a `Connected` worker with matching arch; registers a `worker_sender`
   whose receiver was dropped; calls `try_dispatch(&state)`; asserts
   `Err(DispatchError::Send(_))`, DB state `queued`, all six WCP D4 provenance
   columns NULL, and the build re-enqueued at the front of its priority lane.

**Estimated cost:** ~80 LOC of test code (no production change).

---

#### Commit 7 — `cbsd-rs: enforce strict CBSD_DEV parsing and loopback dev-mode`

**Closes** audit-rem D1, audit F1.

**Capability**: setting `CBSD_DEV` to anything other than `1` / `true` / `yes` /
`on` (case-insensitive) no longer silently enables the worker's `NoVerifier`
rustls bypass. The worker refuses to start when dev mode is active AND the
configured `server_url` is non-loopback.

**Packages**: new `cbsd-common` crate (workspace member) for the shared
`is_truthy_env` helper. `cbsd-server` and `cbsd-worker` consumers. The
`is_loopback_url` predicate lives in `cbsd-worker` (per-binary concern).

**Notable pitfalls**:

- The `is_loopback_url` algorithm MUST operate on the parsed `url::Host`, not on
  a raw string prefix. A naive `starts_with("wss://localhost")` admits
  `wss://localhost@evil.com/`. WCP-style three-way match (`Host::Domain`
  ASCII-case-insensitive "localhost" + `Ipv4::is_loopback()` +
  `Ipv6::is_loopback()`).
- Startup `WARN` log when dev mode is active MUST NOT echo the raw `CBSD_DEV`
  value (could be a misconfigured secret); only a boolean.
- Add `cbsd-common` to `cbsd-rs/Cargo.toml` workspace members.
- Audit-rem revision-history confirmed that `cbsd-common` doesn't exist today
  and the helper has no production callers yet — introducing the crate alongside
  its first users keeps the commit smell-test clean.

**Tests**: `is_truthy_env` unit tests covering accepted set, rejected set,
empty, unset, malformed; `is_loopback_url` 8 tests including authority-confusion
negatives; worker startup test that `CBSD_DEV=false` does not install
`NoVerifier`; worker refuses to start with dev mode + non-loopback URL.

---

#### Commit 8 — `cbsd-rs/worker: enforce tarball containment and decompression cap`

**Closes** audit-rem D5, audit F7.

**Capability**: the worker tar unpack rejects symlinks whose resolved target
escapes the unpack root, rejects PAX-overridden paths with `..` components,
rejects device/fifo/escaping-hardlink entries, and aborts unpacks that exceed
`MAX_UNCOMPRESSED_BYTES = 256 MiB`.

**Packages**: `cbsd-worker` (`build/component.rs` unpack rewrite).

**Notable pitfalls**: see audit-rem D5 prose. Key items: use PAX-aware
`entry.path()`/`entry.link_name()`; two-phase containment with phase 2 as TOCTOU
defense; `path-clean`-style strict logical normalization; keep the legitimate
`components/ceph/containers/v20.3 → ./v20.2` symlink working; consider
`safer-unpack` crate as alternative implementation.

**Tests**: ~10 fixtures covering happy-path symlink, absolute-target symlink,
relative-escape symlink, PAX-overridden path, chained- symlink attack (phase 2
fault injection), `path-clean` regression, device entry, escaping hardlink,
gzip-bomb at byte cap, boundary test at exactly the cap.

---

#### Commit 9 — `cbsd-rs: cap REST body and WebSocket message sizes`

**Closes** audit-rem D6, audit F8.

**Capability**: REST endpoints reject bodies > 1 MiB with 413; WS connections
enforce `WS_MAX_MSG = 8 MiB` / `WS_MAX_FRAME = 1 MiB` on both server-accept and
worker-connect paths.

**Packages**: `cbsd-server` (`main.rs` router builder `RequestBodyLimitLayer` +
WS accept config), `cbsd-worker` (`ws/connection.rs` connect config).

**Notable pitfalls**:

- Tarball binary frame must fit within `WS_MAX_MSG`. Real components pack to ~2
  KiB today; 8 MiB is ~4000× headroom.
- No per-log-line cap is added — D6's trust argument is that authenticated
  workers are trusted to emit free-form text. The argument is fully in force
  only after Phase 1's W2 (ownership checks) lands; the plan orders Phase 1
  before Phase 2, so D6's trust position is sound at commit 9.
- `tower_http::limit::RequestBodyLimitLayer` is the axum-side layer.

**Tests**: REST > 1 MiB → 413; WS message > `WS_MAX_MSG` → protocol- level
close; tarball binary frame just under/over `WS_MAX_MSG`.

---

#### Commit 10 — `cbsd-rs/server: reject OAuth callback when email_verified is false`

**Closes** audit-rem D2, audit F2.

**Capability**: a Google account with `email_verified: false` can no longer log
in. The check runs before allowed-domain check so an attacker cannot probe
domain allow-lists with unverified accounts.

**Packages**: `cbsd-server` (`routes/auth.rs` OAuth callback handler).

**Notable pitfalls**:

- Generic error to the user ("authentication failed; contact your
  administrator"). Server-side log records email, provider response, reason. Do
  NOT leak domain-allowed vs verification-failed in the user-facing error.
- `serde(alias = "verified_email")` for the legacy v1 field name.
- Residual trust gap: the `userinfo` REST call is not bound to the OAuth
  signature. Future ID-token-introspection work would close this; out of scope
  for this commit.

**Tests**: 4 mocked-userinfo tests (false → 401; true + allowed → 200; missing →
401; legacy alias accepted).

⚠ Size: ~130 LOC, below the 200 floor. Justified — focused security fix;
combining with another commit would dilute review focus.

---

#### Commit 11 — `cbsd-rs/server: split periodic:manage into :own and :any caps`

**Closes** audit-rem D3 (part 1), audit F4 (write-path).

**Capability**: `periodic:manage:own` holders can mutate only their own periodic
tasks; `periodic:manage:any` is the admin variant. Cross-owner mutation by
`:own` holders → 403.

**Packages**: `cbsd-server` (capability constants, route handlers, seed
migration).

**Notable pitfalls**:

- Migration drops legacy `periodic:manage` from every role's capability set. No
  auto-mapping. Migration SQL comment must call this out for operators with
  custom roles.
- All four mutating endpoints (`update_task`, `delete_task`, `enable_task`,
  `disable_task`) need the `:any` OR `:own + owner- match` check.
- Descriptor updates re-validate scopes against the **updating user's**
  effective scopes (not the row owner's). This is the write-time half of F4. The
  trigger-time half is commit 12.

**Tests**: 6 RBAC tests + 1 custom-role migration test (per audit-rem D3 test
list).

---

#### Commit 12 — `cbsd-rs/server: re-validate periodic task scopes at trigger time`

**Closes** audit-rem D3 (part 2), audit F4 (trigger-path), SI-15.

**Capability**: scheduled triggers re-validate the stored descriptor against the
task owner's current effective capabilities. Lost capabilities or missing owner
row → task disabled, `last_error = owner_account_missing`.

**Packages**: `cbsd-server` (`scheduler/trigger.rs` user-lookup helper).

**Notable pitfalls**:

- Today's hard-delete schema → canonical lookup is `WHERE email = ?` unfiltered.
  Do NOT add a soft-delete filter against today's schema (column doesn't exist).
  The soft-delete clause is forward- protective per audit-rem D3 conditional
  contract.
- The owner-deleted test (`D3-T-owner-deleted`) covers hard delete. The
  `D3-T-owner-soft-deleted` test is feature-gated behind
  `cfg(feature = "soft-delete-schema")` and runs an inline
  `ALTER TABLE users ADD COLUMN deleted_at TIMESTAMP NULL` in its setup — do NOT
  fork the migrations directory.
- Trigger MUST NOT panic, MUST NOT raise to the scheduler loop, MUST NOT fall
  back to cached scopes. The scheduler must continue firing other tasks after
  one task is disabled.

**Tests**: trigger-time scope-reduction; D3-T-owner-deleted (hard);
feature-gated D3-T-owner-soft-deleted; scheduler-loop-continuity.

---

#### Commit 13 — `cbsd-rs: redact bearer tokens from URI logging surface`

**Closes** audit-rem D4 + D9, audit F5.

**Capability**: CLI login flow no longer leaks the PASETO token to server access
logs. The project-wide URI-logging policy prevents any endpoint from
accidentally leaking secrets via path/query log fields.

**Packages**: `cbsd-server` (`routes/auth.rs` redirect, TraceLayer config,
panic-handler config), `cbsd-rs/ui/index.html` (read from hash +
`history.replaceState`), `cbsd-rs/CLAUDE.md` (correctness invariants note).

**Notable pitfalls**:

- One-character server fix: `?cli-token=…` → `#cli-token=…` in the redirect URL.
- UI MUST call `history.replaceState({}, '', '/')` **immediately** after reading
  the token from `window.location.hash`, before any other script runs. Order
  matters.
- TraceLayer span MUST emit `method`/`path` (`Uri::path()`) only — never `query`
  or full `Uri::to_string()`. Policy applies to every middleware in the stack
  including panic handlers and any future error-reporting integration.

**Tests**: redirect test asserts `Location: /#cli-token=…`; negative log-capture
test on `?cli-token=…` request; 2 browser-level tests for hash-clear +
history-clear.

---

#### Commit 14 — `cbsd-rs: wrap token material in Secret<T>`

**Closes** audit-rem D10 (wrap half), audit F13 by-construction guarantee.

**Capability**: every in-memory token (PASETO raw tokens, API keys, robot
tokens, worker tokens) is wrapped in `secrecy::Secret<T>`. Accidental
`#[derive(Serialize)]` over a `Secret<T>` field fails to compile. All
inner-value access goes through `.expose_secret()` (a named accessor, grep-able
and auditable).

**Packages**: workspace `Cargo.toml` (add `secrecy = "0.10"`), `cbsd-proto`
(`WorkerToken.api_key`), `cbsd-server` (PASETO + OAuth

- robot tokens), `cbsd-worker` (stored API key), `cbsd-rs/cbc` (persisted bearer
  in `Config`).

**Notable pitfalls**:

- `secrecy::ExposeSecret` is a **trait**, not an inherent method. Every
  `.expose_secret()` call site MUST `use secrecy::ExposeSecret;` to bring the
  trait into scope.
- Wire types that today derive `Serialize` and include token fields fail to
  compile after the wrap. Either replace the derive with a custom `Serialize`
  calling `.expose_secret()` at the wire boundary, or restructure to separate
  the wire DTO from the in-memory secret holder.
- The CI grep gate is deferred (roadmap item). This commit ships the wrapper +
  its users, not the gate.

**Tests**: 1 `tracing-test` redaction test; 1 `trybuild` compile-fail for
`#[derive(Serialize)]` over `Secret<String>`; 1 `trybuild` compile-fail for
inner-field access without `.expose_secret()`.

---

#### Commit 15 — `cbsd-rs: redact token material from tracing call sites`

**Closes** audit-rem D10 (audit half), SI-10.

**Capability**: every existing `tracing::*!` macro call across the workspace
that previously emitted token material is updated to route through `Secret<T>`
(covered in commit 14) or to emit a non- reversible per-process diagnostic
identifier instead of key bytes.

**Packages**: `cbsd-server`, `cbsd-worker`, `cbsd-rs/cbc`, `cbsd-proto`
(grep-and-fix sweep).

**Notable pitfalls**:

- API key prefix logging at debug (F13's original site) is replaced with a
  stable per-process diagnostic identifier derived from the key hash. The
  identifier must not be reversible by a log reader.
- `signed_off_by` and similar non-secret identity fields are explicitly NOT in
  scope.

**Tests**: targeted `tracing-test` assertions per fixed site.

⚠ Size: ~200 LOC, at the floor. Justified — targeted policy enforcement; not
architecturally splittable.

---

#### Commit 16 — `cbsd-rs/server: index api_keys.key_prefix for O(log n) lookups`

**Closes** audit-rem D7, audit F10.

**Capability**: API key prefix lookups are O(log n) and the timing side-channel
exposed by the missing index is closed.

**Packages**: `cbsd-server` (`migrations/` + `.sqlx/` cache).

**Notable pitfalls**:

- Non-unique B-tree index. Prefix is a UX helper, not a unique key.
- Run `cargo sqlx prepare --workspace` after the migration; the `.sqlx/` delta
  is excluded from the LOC budget per the skill.
- Document the query-plan check in the migration's comment block.

**Tests**: migration apply test (forward + idempotent rerun).

⚠ Size: ~50 LOC, well below the floor. Justified — single SQL migration + cache
regeneration. Combining with other commits would mix unrelated topics.

---

#### Commit 17 — `cbsd-rs/cbc: enforce HTTPS host and write config atomically`

**Closes** audit-rem D8, audit F11.

**Capability**: `cbc` rejects `http://` hosts unless `--insecure-http` is
explicitly set (independent of `-k`/`--no-tls-verify`). `Config::save` writes
atomically with mode `0o600` via temp-file + rename.

**Packages**: `cbsd-rs/cbc` (`client.rs` URL parsing, `main.rs` flag,
`config.rs` atomic save).

**Notable pitfalls**: see audit-rem D8. Key items: `--insecure-http` is
independent of `--no-tls-verify`; per-command warning when `--insecure-http` is
set; `OpenOptions::create_new(true).mode(0o600)` for the temp file; `fs::rename`
over target; best-effort temp-file cleanup on error; Windows non-atomic-rename
caveat documented but not blocking (cbc is Linux-primary).

**Tests**: scheme rejection (https accepted, http/ftp rejected);
`--insecure-http` permits http with warning; atomic-save race test; error-path
temp-file cleanup.

---

#### Commit 18 — `cbsd-rs/proto: add SI-18 regression test for ServerMessage`

**Closes** audit-rem D13-T6 (forward-protection for SI-18), SI-18.

**Capability**: `cbsd-proto` has a regression test that catches any addition of
`#[serde(deny_unknown_fields)]` on `ServerMessage` or any of its variants.
Adding a new `ServerMessage` variant cascades through four automated gates
(witness, tag-enum, as_wire, sentinel_for_tag) before reaching the runtime
case-coverage gate.

**Packages**: `cbsd-proto` (new `[dev-dependencies]` section with
`strum = { version = "0.26", features = ["derive"] }`; `src/ws.rs::tests`
additions).

**Notable pitfalls**:

- The existing `mod tests` block already imports `use super::*;` plus `Arch` and
  a set of build types. The new sketch ONLY adds
  `use serde_json::{Value, json};` and `use strum::IntoEnumIterator;`. Do NOT
  re-import types already in scope.
- The companion enum `ServerMessageTag` derives `strum::EnumIter`, `Debug`,
  `Clone`, `Copy`, `PartialEq`, `Eq` — drop the previously- derived `Hash` per
  v8 review NF-2-v8 (unused).
- Same-crate placement is load-bearing: preserves exhaustive-match semantics if
  `ServerMessage` ever gains `#[non_exhaustive]`.

**Tests**: this commit IS the test.

---

### Phase 3 — WCP-extensions (audit-remediation D11/D12/D13)

#### Commit 19 — `cbsd-rs/worker: report Building during accepted-phase reconnect`

**Closes** audit-rem D11, WCP v10 review open item #1.

**Capability**: the supervisor (from commit 6) extends its reconnect-status
rule: `Building { build_id }` is reported whenever any non-terminal local
assignment state exists, **including the `accepted` phase**.

**Packages**: `cbsd-worker` (supervisor reconnect rule extension).

**Notable pitfalls**:

- Spawn-race case (supervisor cannot determine whether a child process exists):
  follow WCP v11 rule — stop and await any possible child, clean up, only then
  report `Idle`. The `accepted`-only case (no child yet spawned) reports
  `Building` after the supervisor confirms no executor exists.
- Server-side handling of the new `accepted`-phase Building report: the server
  treats it as authoritative receipt of `build_accepted` (mark
  `ReceivedByWorker` per the receipt state from commit 2, cancel the
  dispatch-ack timer, keep the assignment under the new connection).

**Tests**: accept + drop + reconnect → `Building`; spawn-race wait/ kill before
status report.

---

#### Commit 20 — `cbsd-rs/server: resolve dead workers by DB state and receipt`

**Closes** audit-rem D12, WCP v10 review open item #2.

**Capability**: when the liveness monitor declares a worker dead, the resolution
is table-driven over (`builds.state` × `receipt`):
`dispatched + AwaitingReceipt` → roll back to `queued` (via the rollback
function from commit 1); `dispatched + ReceivedByWorker` → `failure` (no requeue
— avoids duplicate S3/Harbor side effects); `started` → `failure`; `revoking` →
`revoked`.

**Packages**: `cbsd-server` (`ws/liveness.rs`, `ws/handler.rs`
`handle_worker_dead`).

**Notable pitfalls**:

- `ReceivedByWorker + dispatched + dead` → `failure`, not requeue. The
  conservative choice — the build may have produced upstream side effects
  already.
- Receipt state is in-memory only (per WCP SI-25). Server restart recovery uses
  the existing fail-in-flight policy and does NOT reconstruct
  `ActiveAssignmentReceipt`.
- The 4-row table is the normative contract; the implementation helper that maps
  (state, receipt) → action should be a single match.

**Tests**: 5 dead-worker resolution tests (one per row + server- restart
interaction).

---

#### Commit 21 — `cbsd-rs: deliver migration revoke and drain terminal-pending`

**Closes** audit-rem D13, WCP v10 review open item #3.

**Capability**: same-worker reconnect migration sends
`BuildRevoke { reason: Some(MigrationSupersede) }` on the old sender for every
owned active assignment **before** removing the old sender. The worker
supervisor confirms migration via a `last_authenticated_connect_at` predicate
within `MIGRATION_RECENT_WINDOW = 30s`. If `terminal-pending-report` is the
local state at migration time, the supervisor **drains** the real outcome and
reports `build_finished(<actual_outcome>)` rather than discarding.

**Packages**: `cbsd-proto` (extend `BuildRevoke` with
`reason: Option<BuildRevokeReason>`; add `BuildRevokeReason` enum),
`cbsd-server` (migration revoke send), `cbsd-worker` (supervisor predicate +
drain-then-revoke semantics + `tokio::time::Instant` clock injection).

**Notable pitfalls**:

- **Compile-break sites on adding `reason` to `BuildRevoke`.** `BuildRevoke` is
  a struct variant in `cbsd-proto/src/ws.rs:38`
  (`BuildRevoke { build_id: BuildId }`). Every current consumer uses named-field
  syntax without `..`, so adding the field — even as `Option<>` with
  `#[serde(default)]` — is a hard compile break that must be repaired at every
  site in the same commit. The compiler catches these immediately since the
  packages list includes all three crates; this pitfall exists so a reviewer can
  confirm the diff touches every site, not just the worker destructure. Two
  earlier commits add further `BuildRevoke` sites that this commit must also
  update: commit 2 introduces reporter-directed `BuildRevoke` cleanup paths, and
  commit 18 introduces a `sentinel_for_tag` test helper that constructs
  `BuildRevoke` as a named-field struct literal (see audit-rem design lines
  2347-2363). Sites that exist or will exist by the time this commit lands:
  - Server-side struct-literal constructions verified against source on the
    current branch (three sites):
    - `cbsd-server/src/ws/dispatch.rs:500`:
      `let msg = ServerMessage::BuildRevoke { build_id: BuildId(build_id) };`
    - `cbsd-server/src/main.rs:422`:
      `let msg = cbsd_proto::ws::ServerMessage::BuildRevoke { build_id: cbsd_proto::BuildId(*build_id) };`
    - `cbsd-server/src/ws/handler.rs:698`:
      `let msg = ServerMessage::BuildRevoke { build_id };`
  - Worker-side destructure verified against source on the current branch (one
    site, named-field pattern without `..`):
    - `cbsd-worker/src/ws/handler.rs:389`:
      `ServerMessage::BuildRevoke { build_id } => { … }`
  - Test-side struct-literal construction added by commit 18 (one site):
    - `cbsd-proto/src/ws.rs::tests::sentinel_for_tag` — per audit-rem design
      lines 2361-2363, the `ServerMessageTag::BuildRevoke` arm constructs
      `ServerMessage::BuildRevoke { build_id: BuildId(0) }`. The design notes
      the underlying types do not impl `Default`, so the field must be filled
      explicitly:
      `ServerMessage::BuildRevoke { build_id: BuildId(0), reason: None }`.
      Commit 21 lands after commit 18, so this site is live by the time this
      commit applies. Reporter-directed `BuildRevoke` constructions added in
      commit 2 will also surface in this commit; they pick up the new field
      naturally on the diff.
- `reason` field MUST be `Option<BuildRevokeReason>` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`. This is what
  makes the addition forward- and backward-compatible. SI-18 (no
  `deny_unknown_fields`) is the protective invariant — commit 18 (`cbsd-proto`
  test) is the regression gate. Both must be in place.
- Worker MUST use `tokio::time::Instant` (not `std::time::Instant`) for
  `last_authenticated_connect_at` so test code with `tokio::time::pause()` can
  drive the predicate.
- Anti-coercion: worker checks `migration_plausible()` before honouring
  `MigrationSupersede`. Malicious-server case where `MigrationSupersede` arrives
  without a real migration falls through to `Admin` semantics + WARN log +
  non-fatal counter.
- Drain-then-revoke deviation from WCP v11's generic revoke rule is SCOPED to
  `reason = MigrationSupersede` only. Admin revokes against
  `terminal-pending-report` continue to discard per WCP.
- `Instant` does NOT advance during host suspension on Linux (`CLOCK_MONOTONIC`
  semantics). Documented in SI-17; operator guidance is "don't run cbsd-worker
  on a suspending host."

**Tests**: 7 same-worker migration tests (4 SM-W phases × 3 edge cases); 5
serde-compatibility tests on `BuildRevoke.reason` (absent field, None
round-trip, Some round-trip, unknown-variant rejection, advisory fallback);
D13-T7 boundary-test sub-cases using `tokio::time::pause()` for deterministic
timing.

This is the largest commit in the plan (~700 LOC). At the upper end of the
400-800 budget. Splitting would create non-compiling intermediates (e.g., adding
`BuildRevokeReason` without any users).

---

## Open questions / requested exceptions

Per the `git-commits` skill, exceptions to the 400-800 LOC guideline must be
justified and surfaced. The exceptions in v1 of the plan remain (the user
accepted all in v1); v2 adds two more from the Phase 1 additions.

**Carried from v1 (already accepted by the user)**:

- Commit 10
  (`cbsd-rs/server: reject OAuth callback when email_verified is false`) at ~130
  LOC — below floor; focused high-severity security fix. ✓ accepted.
- Commit 15 (`cbsd-rs: redact token material from tracing call sites`) at ~200
  LOC — at floor; targeted audit pass. ✓ accepted.
- Commit 16 (`cbsd-rs/server: index api_keys.key_prefix`) at ~50 LOC — well
  below floor; single migration. ✓ accepted.
- Commit 21 (`cbsd-rs: deliver migration revoke and drain terminal-pending`) at
  ~700 LOC — at upper bound; splitting creates non-compiling intermediates. ✓
  accepted.
- D11 in Phase 3 rather than Phase A — accepted reclassification. ✓ accepted.
- `cbsd-common` crate introduction in commit 7 — accepted.
- `secrecy = "0.10"` version pin — accepted.

**New in v2 (Phase 1 additions)**:

8. **Commit 4 (`cbsd-rs/server: reject empty components arrays`) at ~180 LOC** —
   below the 200-LOC floor.
   - Justification: a focused validator module covering REST + periodic +
     scheduler trigger paths. Combining with adjacent Phase 1 commits would mix
     concerns (validator vs ownership vs rollback). The capability ("descriptors
     with empty components no longer enter the queue") is clear and standalone.
   - Recommended action: accept as-is.

9. **Commit 2 (`cbsd-rs: enforce build-scoped authorization`) at ~700 LOC** —
   within range but at the upper end. Could split between `cbsd-proto` (new
   protocol types) + `cbsd-server` (handler updates).
   - Justification against splitting: the new `UnauthorizedBuildAction` protocol
     type has no callers without the handler updates. Per the `git-commits`
     skill's "library + consumer" split-point rule, this is precisely the case
     where splitting creates a dead-code commit on one side.
   - Recommended action: accept as-is. Alternative: split as proto-first +
     handler-second, accepting the dead-code commit 1 for protocol — please
     indicate preference.

10. **Commit 6 (`cbsd-rs/worker: introduce process-level supervisor`) at ~750
    LOC** — within range but at the upper end.
    - Justification against splitting: the supervisor is structurally one
      feature. The websocket loop today owns the active build state; the
      supervisor extracts this. Splitting would require either (a) the
      supervisor + websocket loop both holding state transiently (broken
      intermediate), or (b) the supervisor existing with no users (dead code).
      Neither passes the smell test.
    - Recommended action: accept as-is.

## Test strategy

Per the `git-commits` skill, tests land with their implementing commit. The
plan's LOC budgets include tests. Per-commit test counts are listed under each
commit's "Tests" line.

Aggregate test additions across the plan: ~50 unit tests + ~25 integration
tests + ~10 `trybuild` or browser-level tests. Detailed test inventory lives in
the two design documents under "Test Expectations" / "Test Expectations
Summary".

Per `cbsd-rs/CLAUDE.md` pre-commit policy, every commit MUST pass
`cargo fmt --all`, `cargo clippy --workspace -- -D warnings`, and
`cargo check --workspace` (with `SQLX_OFFLINE=true` if needed) before staging.

## References

- WCP design (authoritative):
  `cbsd-rs/docs/cbsd-rs/design/019-20260426T1154-worker-control-plane-hardening.md`
  (Draft v11).
- Audit-remediation design:
  `cbsd-rs/docs/cbsd-rs/design/019-20260514T1040-security-audit-remediation.md`
  (Draft v8).
- WCP soundness review:
  `cbsd-rs/docs/cbsd-rs/reviews/019-20260516T0952-design-wcp-soundness-v1.md`.
- Prior plan v1:
  `cbsd-rs/docs/cbsd-rs/plans/019-20260516T0758-security-audit-remediation.md`.
- Audit reviews:
  `cbsd-rs/docs/cbsd-rs/reviews/019-20260512T2339-impl-cbsd-rs-security-audit-v1.md`,
  `…-v1.1.md`.
- Original security review: `cbsd-rs/docs/000-20264026T1104-security-review.md`.
- Roadmap: `cbsd-rs/docs/ROADMAP.md`.
- Audit-remediation design v8 review (last in the cycle, score 90/100):
  `cbsd-rs/docs/cbsd-rs/reviews/019-20260516T0715-design-security-audit-remediation-v8.md`.
