# 011 — Worker integration (in-process cutover) — design review v2

Second-round adversarial review of
`cbsd-rs/docs/cbscore-rs/design/011-20260622T1359-worker-integration.md` (the
final design of the cbscore→Rust port set: the worker's Python-subprocess →
in-process `cbscore::runner::run` cutover). Round v1 returned NO-GO / 32-100
with two blockers (F1, F2), three majors (F3–F5), and a nit (N1). This round
verifies the v1 fixes against the actual worker source and re-hunts for new
defects the rework introduced.

Method: every claim was re-traced into the live worker source
(`cbsd-worker/src/build/{executor,output,supervisor,component}.rs`,
`ws/handler.rs`), the retired contract (`scripts/cbscore-wrapper.py`), the
contracts it builds on (009, 006, 004, 001), and the workspace manifests. The
implementer is not trusted; line references below are first-hand.

## Verdict

**NO-GO as written / conditional.** The five substantive v1 findings (F2–F5) and
the nit (N1) are genuinely and correctly resolved, and the v1 panic-seam _wedge_
(the heart of F1) is fixed: the rework makes a terminal flow on **every** build
outcome — including a panic — so the handler observes it, `retire()` fires, and
the worker no longer wedges "busy". That is real progress and it lands the B2
acceptance gate's liveness property.

But the rework **introduced a new soundness gap on the same seam** (F1-b): the
document's own ownership model for the build-task `JoinHandle` is
self-contradictory, and the one consequence the doc explicitly asserts —
`shutdown()` "awaits the completion path so the container is torn down before
the worker exits" — is **unbacked** as written. A `tokio::JoinHandle` is
awaitable exactly once; the doc simultaneously (a) stores the build-task handle
in the supervisor's `ActiveBuild` record via `register_accepted`, and (b) has
the completion path own and await that same handle exactly once, and (c) has
`shutdown()` await "the completion path" — but never says the completion path is
a spawned task, never says where _its_ handle is stored, and stores the
build-task handle (not a completion-task handle) in the record. Under any
consistent reading, either the completion path or `shutdown()` is left with no
handle to await. Because this sits on the named B2 failure-isolation gate (001
invariant 6) and governs whether an active build's container is reliably stopped
at worker exit, it must be pinned down before implementation. One new minor race
(F6, the normal-completion vs operator-revoke terminal race) is noted but does
not block.

Fix F1-b and re-review; F6 is accept-or-amend.

## Status of each v1 finding

| v1 finding                                | Status                                    | Evidence                                                                                                                             |
| ----------------------------------------- | ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| F1 — panic seam contradictory/unreachable | **Partly resolved**; new sub-finding F1-b | Wedge fixed (terminal always flows → `retire()` fires). Handle-ownership model still contradictory; `shutdown()` await unbacked.     |
| F2 — `panic = "unwind"` not pinned        | **Resolved**                              | Doc now mandates pinning in the workspace `[profile.release]` (011:174–178); no `[profile]` block exists in any manifest (verified). |
| F3 — kill→cancel omits two spool sites    | **Resolved**                              | All four sites named (011:197–199 → supervisor.rs :235/:503/:627/:661); first-terminal-wins arbitration specified and sound.         |
| F4 — 7200 s timeout default disappears    | **Resolved**                              | Worker now injects 7200 s when unset (011:86–87, 270–274); matches wrapper:233; executor.rs:150–152 sets the env only when `Some`.   |
| F5 — 64 KiB report cap dropped            | **Resolved**                              | Cap re-homed to the completion path (011:137–141); output.rs:35 = 65 536.                                                            |
| N1 — config/translate step order inverted | **Resolved**                              | Config-load is step 1, translation step 2; the data dependency is stated explicitly (011:63–67).                                     |

## What the rework gets right (verified this round)

- **F2 pin is now mandated, and homed correctly.** 011:174–178 says the cutover
  "**must pin `panic = "unwind"`** explicitly in the workspace
  `[profile.release]`", calling out that "today no `[profile]` block exists" and
  that 001's failure-isolation invariant requires the pin "so a future
  `panic = "abort"` cannot silently convert a contained build panic into a
  worker-wide abort." This is exactly what 001 invariant 6 / B2
  (001:112–115, 290) demands. Verified: no `[profile…]` block in
  `cbsd-rs/Cargo.toml` (the only legal home for a workspace profile) nor in any
  of the five member manifests (`cbc`, `cbsd-common`, `cbsd-proto`,
  `cbsd-server`, `cbsd-worker`). Cargo honours `[profile]` only at the workspace
  root, so "workspace `[profile.release]`" is the correct location.

- **F3 — all four kill sites, sound arbitration.** The four `exec.kill()`
  callers are confirmed in the live supervisor: `on_build_revoke`
  (supervisor.rs:235), `shutdown` (:503), spool-overflow (:627), and
  spool-write-error (:661). 011:197–199 enumerates all four → `token.cancel()`.
  The first-terminal-wins rule (011:202–211) is sound against the real code:
  both spool sites set `spool_exhausted = true` (supervisor.rs:616, 659) and
  synthesize an authoritative `Failure` `pending_terminal` (633, 666) **before**
  cancelling; `spool_exhausted` is sticky (set in `spool_and_finalize`, never
  cleared by the reconnect drain at :403–409), and the early-return guard at
  :587–590 drops any later `on_output_message`. So the completion path's
  would-be `Revoked` (from the resulting `RunnerError::Cancelled`) is suppressed
  and the spool `Failure` stays authoritative. The doc explicitly states the
  completion path checks `spool_exhausted`/`pending_terminal` and suppresses
  (011:206–211), which matches the mechanism.

- **F4 — 7200 s default re-homed into the worker.** 011:84–88 and 270–274 now
  have the worker supply `RunOpts.timeout` "from its configured build timeout
  (default 7200 s … to preserve the wrapper's `CBS_BUILD_TIMEOUT` default)" and
  "never falls through to 009's 4 h default". This is the parity fix v1
  demanded: wrapper:233 is `int(os.environ.get("CBS_BUILD_TIMEOUT", "7200"))`,
  and the worker only set the env when `config.build_timeout_secs` was `Some`
  (executor.rs:150–152), so an unconfigured worker would otherwise inherit 009's
  4 h (009:171, 276). The worker now owns the 7200 s fallback.

- **F5 — 64 KiB cap re-homed.** 011:137–141 has the completion path "check the
  serialized size and drop an over-cap report (logging a warning), exactly as
  today" before placing it in `BuildFinished`, on both success and failure paths
  (011:306–307). output.rs:35 confirms `MAX_REPORT_SIZE = 65_536`. The doc
  correctly notes the typed path removes only the sentinel parser, not the
  guard.

- **N1 — step order fixed.** 011:63–67 loads config first (step 1) and states
  the dependency outright: "Config loads **first** because translation (step 2)
  reads `config.storage.registry`." This matches the wrapper, which loads config
  at :142 before `version_create_helper` at :167, and reads
  `config.storage.registry.url` (:182) and `config.paths.components` (:152,
  :174) out of the loaded config.

- **F1 wedge is genuinely fixed.** v1's killing objection was that a panicking
  build task sent no `BuildFinished`, so `retire()` (which fires only after the
  handler observes a terminal on the outbound channel — handler.rs:98–104,
  217–233) never ran, `active` was never cleared, and the worker stayed "busy"
  forever. The v2 model (011:159–169) makes the completion path the **sole**
  terminal producer and emits a synthesized `Failure` on `is_panic()` "via the
  same path" (`on_output_message`). So a panic now yields a terminal → handler
  observes it → `retire()` fires → `active` clears → the next `BuildNew` is
  accepted. The liveness half of the B2 gate holds.

- **Descriptor-translation, native-result, run-name, and crash-containment
  mechanism** remain accurate (re-confirmed against wrapper :58–63, :127–128,
  :148–149, :152, :167–187, :234, and 009's `RunnerError`/`podman_stop`
  contract); these were correct in v1 and are unchanged.

- **`component.rs` retained.** The doc claims tarball validate/unpack/cleanup is
  kept unchanged (011:42–47, 250). Confirmed: `component::validate_and_unpack` /
  `component::cleanup` are orthogonal to the execution mechanism and untouched
  by the cutover.

## Findings (ordered by severity)

### F1-b — BLOCKER (new): completion-path handle ownership is self-contradictory; `shutdown()`'s container-teardown await is unbacked (confidence 88)

The v2 rework fixes the wedge but leaves the build-task `JoinHandle` ownership
under-specified in a way that contradicts itself. A `tokio::JoinHandle` is
non-cloneable and can be `.await`ed exactly once. The document makes three
claims that cannot all hold for one handle:

1. **The supervisor record stores the build-task handle.** `register_accepted`
   "takes the token and the build-task `JoinHandle`" (011:195–196), and the
   field substitution is "`executor: Option<BuildExecutor>` becomes
   `cancel_token + build-task JoinHandle`" (011:281–283). So the build-task
   handle lives in the `ActiveBuild` record.
2. **The completion path owns and awaits that handle exactly once.** "A
   **completion path** awaits that handle exactly once and is the only place a
   terminal is produced" (011:159–160); "single-owner build-task `JoinHandle`"
   (011:152, 260).
3. **`shutdown()` awaits the completion path.** "`shutdown()` cancels the token
   (below) and awaits the completion path so the container is torn down before
   the worker exits" (011:184–185); the `Revoking` phase "awaits the completion
   path plus token" (011:216–217).

To honour (2), the completion path must take ownership of the single handle —
i.e. **move it out of the record** (exactly as today's `retire()` does with
`output_task` at supervisor.rs:451, and `shutdown()` at :507). Once moved, the
record holds nothing, so (1)'s "stored in the record" handle and (3)'s
"`shutdown()` awaits" have nothing to await. Conversely, if the handle stays in
the record so `shutdown()` can await it, the completion path is not its single
owner and (2) is false.

The doc never resolves this because it never specifies the completion path as a
concrete object: it is never described as a `tokio::spawn`, its own `JoinHandle`
is never named, and nothing is said about storing a _completion-task_ handle in
the supervisor (the slot the old `output_task` occupied). The grep is
unambiguous — `tokio::spawn` appears only for the **build task** (011:155), and
every "completion path" mention treats it as an abstract "place", never a
spawned task with a stored handle. So the single-owner guarantee is **asserted
but not pinned**, which is precisely the failure mode the v1 fixes were warned
against reintroducing.

The operational consequence is concrete and on the B2 gate. `shutdown()` today
(supervisor.rs:496–539) is the worker's only clean stop path: it cancels, then
**awaits the task and the executor** so the builder container is torn down
before the process exits. The doc keeps this requirement (011:184–185) but,
under the contradiction above, there is no specified handle for `shutdown()` to
await. If the implementer resolves the contradiction by having the completion
path own the handle (the natural reading of "single owner"), `shutdown()` can
only fire the token and return — it cannot await the in-flight `podman_stop` /
`runner::run` unwind, so the worker can exit while the builder container is
still being stopped. That is a container-leak / unclean-exit regression versus
today's await-the-child `shutdown()`.

**Required.** Specify the completion path concretely: it is a `tokio::spawn`'d
task that (a) is the single owner of the build-task `JoinHandle` (the build
task's handle is **moved into** the completion task at spawn/register time, not
stored in the record), (b) awaits it once, maps the outcome (or synthesizes the
panic `Failure`) and emits the terminal via `on_output_message`. Store the
**completion task's** `JoinHandle` in the `ActiveBuild` record (the slot
`output_task` occupies today), and have `retire()`/`shutdown()` await **that**
handle — so `shutdown()`'s "container torn down before exit" guarantee is
actually backed by an awaited task. Then make 011:181–185 and 011:281–283
consistent: the record stores the **completion-task** handle, not the bare
build-task handle, and `register_accepted` should reflect whichever handle the
record actually holds.

### F6 — minor (new): normal-completion vs operator-revoke terminal race is unaddressed (confidence 80)

The first-terminal-wins arbitration (011:202–211) is specified only for the two
**spool** teardown sites (their synthetic `Failure` vs the completion path's
`Revoked`). It does not address the symmetric race on the **operator
`BuildRevoke`** path: a build whose `runner::run` returns `Ok(...)` (mapped to
`Success`) at almost the same instant an operator `BuildRevoke` fires the token.
`on_build_revoke` (supervisor.rs:224–244) transitions to `Revoking`, clears any
`pending_terminal`, and cancels — but if `runner::run` had already returned `Ok`
(the container finished before `podman_stop` took effect), the completion path
maps it to `Success`, not the `Revoked` the operator and the supervisor's
`Revoking` phase expect. The doc states "only an operator `BuildRevoke` yields
`Revoked`" (011:211) as if the revoke deterministically produces `Revoked`,
which is not guaranteed under this race.

This race largely pre-exists the cutover (a subprocess could likewise exit 0
between revoke and SIGTERM), so it is not a regression, but the v2 doc newly
leans on a clean "revoke ⇒ `Revoked`" mapping and should state the tie-break
explicitly: either the completion path checks the `Revoking` phase /
token-cancelled state and downgrades an `Ok` outcome to `Revoked` when a revoke
is in flight, or the doc accepts that a revoke racing a successful finish may
legitimately report `Success` and says so. The doc currently implies determinism
it does not establish.

**Recommended.** State the operator-revoke arbitration rule explicitly,
mirroring the spool-site treatment: define whether `Revoking`/token-fired forces
`Revoked` over a late `Ok`, or document the benign race.

## Confidence score

| Item                                                                    | Points | Description                                                                                                           |
| ----------------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                          | 100    |                                                                                                                       |
| F1-b (D8): completion-path handle ownership self-contradictory          | -15    | Single non-cloneable handle is simultaneously record-stored, completion-path-owned, and shutdown-awaited — can't hold |
| F1-b (D8): `shutdown()` container-teardown await unbacked               | -5     | No specified handle for `shutdown()` to await under the single-owner reading; container-leak/unclean-exit regression  |
| F6 (D8): normal-completion vs operator-revoke terminal race unaddressed | -5     | Doc implies revoke ⇒ `Revoked` deterministically; a late `Ok` can win the race; tie-break unspecified                 |
| **Total**                                                               | **75** |                                                                                                                       |

Interpretation: 75 — **acceptable with noted improvements; fix before next
stage.** The score is dominated by F1-b, which sits on the B2 failure-isolation
acceptance gate and must be resolved. The remaining v1 findings (F2–F5, N1) are
all resolved, which is why the score rose sharply from v1's 32.

## Required actions before GO

1. **F1-b** — Specify the completion path as a concrete `tokio::spawn`'d task
   that is the sole owner of the build-task `JoinHandle` (moved in, not record-
   stored). Store the **completion task's** handle in the `ActiveBuild` record
   (the `output_task` slot), and have `retire()`/`shutdown()` await that handle
   so `shutdown()`'s "container torn down before worker exit" guarantee is
   actually backed. Reconcile 011:181–185 and 011:281–283 with this ownership
   (the record holds the completion-task handle; `register_accepted` reflects
   what the record actually holds).
2. **F6** — State the operator-revoke arbitration explicitly: either downgrade a
   late `Ok` to `Revoked` when a revoke/token-cancel is in flight, or document
   the benign `Success`-wins race. Do not imply a deterministic revoke ⇒
   `Revoked` mapping the wiring does not guarantee.
