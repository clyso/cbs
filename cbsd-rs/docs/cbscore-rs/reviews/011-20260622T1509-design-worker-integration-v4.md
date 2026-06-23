# 011 — Worker integration (in-process cutover) — design review v4

Fourth-round adversarial review of
`cbsd-rs/docs/cbscore-rs/design/011-20260622T1359-worker-integration.md` (the
final design of the cbscore→Rust port set: the worker's Python-subprocess →
in-process `cbscore::runner::run` cutover). This is the intended **last** round.
Prior rounds: v1 NO-GO 32/100 (blockers F1, F2; majors F3–F5; nit N1); v2
NO-GO/conditional 75/100 (F1-b blocker; F6 minor); v3 NO-GO/conditional 74/100
(F1-b & F6 resolved; new blocker F1-c; nits N2/N3). This round verifies the v3
fix — the register-first / attach-second ordering restoration — against the live
worker source and re-hunts for any gap the patch introduced.

Method: every claim was re-traced into the live worker source
(`cbsd-worker/src/build/{executor,output,supervisor,component}.rs`,
`ws/handler.rs`), the retired contract (`scripts/cbscore-wrapper.py`), and the
contracts it builds on (009, 001). The implementer is not trusted; line
references below are first-hand. A `grep` over the whole design verified global
consistency of every handle/ownership/token statement.

## Verdict

**GO with nits.** F1-c is genuinely resolved against the source: the doc now
specifies the two-step `register_accepted(build_id, token, component_dir)` →
spawn → `attach_completion_task(build_id, completion_handle)` ordering, which is
byte-for-byte the proven shape today's handler uses (`handler.rs:491-496`
register, `:519-525` attach into the `output_task` slot). With the record set
before the completion task can emit, a fast-failing build's terminal can no
longer hit the orphan-drop guard (`supervisor.rs:270-274`). The
self-contradicting "mirrors today's `output_task` pattern" justification is gone
— the doc now correctly calls `attach_completion_task` "the analog of today's
`attach_output_task`," which is the genuine two-step analog. N2 and N3 are
resolved. The new-gap hunt the task mandated turned up **no blocker**: the
register→attach window cannot be entered by `shutdown()` (it is a sibling
`select!` branch in the same loop that runs `handle_build_new`, including
attach, to completion), `token.cancel()` fires before the skipped await
regardless, and no container can be up before attach. Two nits remain (N4
token-clone plumbing is unspecified; N5 a parity sentence glosses a real but
benign register-window asymmetry); neither blocks, and per the stopping
criterion this round ends the cycle. The "one new blocker per round" pattern is
broken here — the remaining items are exposition, not a fresh correctness
defect, and not a structural problem with the completion-task / register-attach
seam.

## Status of each prior finding

| Finding                                         | Status            | Evidence                                                                                                                                                                                |
| ----------------------------------------------- | ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F1-c — one-step register reopens orphan-drop    | **Resolved**      | Two-step ordering specified (011:178-184); matches `handler.rs:491-496`/`:519-525`; record set before completion task can emit, so `supervisor.rs:270-274` is unreachable on fast-fail. |
| N2 — stale "one supervised build task" singular | **Resolved**      | 011:60-61 now says "**two tasks** … a **build task** … and the **completion task**."                                                                                                    |
| N3 — "the await is immediate" overstated        | **Resolved**      | 011:205-209 now: "the terminal is enqueued on the mpsc, then the task returns), so the await is brief and bounded, not instant."                                                        |
| F1-b — completion-handle ownership / teardown   | **Resolved (v3)** | Re-verified: record stores completion-task handle in `Option`, `.take()`-d once; `shutdown()` barrier sound (completion → build task → `podman_stop`).                                  |
| F6 — revoke/completion terminal race            | **Resolved (v3)** | 011:243-251 documents the benign race; matches today.                                                                                                                                   |
| F2 — `panic = "unwind"` not pinned              | **Resolved (v2)** | 011:197-201 mandates pinning in workspace `[profile.release]`; no `[profile]` block in any manifest (re-verified).                                                                      |
| F3 — kill→cancel omits two spool sites          | **Resolved (v2)** | All four sites named (011:228-230 → `supervisor.rs:235/503/627/661`); first-terminal-wins arbitration sound against `:587-590/:616/:659`.                                               |
| F4 — 7200 s timeout default disappears          | **Resolved (v2)** | Worker injects 7200 s when unset (011:313-317); matches `wrapper:233`.                                                                                                                  |
| F5 — 64 KiB report cap dropped                  | **Resolved (v2)** | Cap re-homed to completion path (011:138-142, 297-299); `output.rs:35 = 65_536`.                                                                                                        |
| N1 — config/translate step order inverted       | **Resolved (v2)** | Config-load is step 1; dependency stated (011:63-68).                                                                                                                                   |

## What the v3 fix gets right (verified this round)

- **F1-c — two-step register-then-attach is restored and matches today
  exactly.** 011:178-184 specifies:
  `register_accepted(build_id, token, component_dir)` "creates the record with
  the token but **no** handle yet; then the handler spawns the build task and
  the completion task; then
  `attach_completion_task(build_id, completion_handle)` stores the handle on the
  existing record (the analog of today's `attach_output_task`)." This is the
  live shape:
  - `register_accepted` (`supervisor.rs:170-194`) sets `state.active` with
    `output_task: None` (`:187`); `attach_output_task` (`supervisor.rs:198-205`)
    fills the slot afterward on the already-present record. The doc's
    `register_accepted(token)` + `attach_completion_task` is a field-for-field
    re-homing (executor → token; output_task → completion-task handle).
  - `handler.rs:491-496` registers **first**, with the load-bearing comment
    _"Register the active build BEFORE spawning the streaming task so the
    supervisor sees the executor in case the streamer produces a message
    immediately."_ — then `:519-525` spawns the producer and calls
    `attach_output_task`. The doc now reproduces precisely this ordering.

- **The orphan-drop window is closed.** Because `register_accepted` sets
  `state.active` before either task is spawned, any terminal the completion task
  emits via `on_output_message` finds a present `state.active`
  (`supervisor.rs:270` `state.active.as_mut()` is `Some`), so the guard at
  `:270-274` cannot fire on the fast-failure paths the doc itself introduces
  (bad `os_version`, missing registry, empty `dst_image.tag`, `Config::load`
  error — 011:74-79, 63-68). 011:184-189 states this consequence explicitly and
  cites `supervisor.rs:270-274`. The "Fast-fail ordering" acceptance gate
  (011:351-354) pins it.

- **The self-contradicting "mirrors today's `output_task` pattern" claim is
  gone.** v3's blocker was a two-adjacent-line contradiction: a one-step
  `register_accepted(token, completion_handle)` justified as "mirroring today's
  `output_task` pattern" (which is two-step). The current text (011:178-184) has
  no one-step form; the only "mirror" claim is that the `Option`-held handle is
  "`.take()`-d for a single await … exactly as today's `retire()` tolerates a
  `None` `output_task`" (011:190-192) — which is accurate (`retire()` `:451`
  takes, `:459` skips `None`).

- **`retire()`/`shutdown()` tolerate a not-yet-attached `None` handle by
  skipping the await.** Re-verified: `retire()`
  `let task = active.output_task.take()` (`:451`) then
  `if let Some(t) = task { … }` (`:459-462`); `shutdown()` `.take()` (`:508`)
  then `if let Some(t) = task` (`:514-516`). Both skip the await cleanly on
  `None`. The doc's claim (011:190-192) holds.

- **Teardown barrier still real (F1-b, re-verified).** `shutdown()`
  (`supervisor.rs:496-539`) cancels under the lock then awaits the stored handle
  outside the lock. Under v4 it cancels the token and awaits the completion-task
  handle; the completion task awaits the build task, which (009:154-167) returns
  only after the cancel-path `podman_stop(name = ctr_name)`. So
  `shutdown().await → completion task → build task → podman_stop` guarantees the
  container is stopped before the worker exits — the in-process analog of
  today's `child.wait()`. 011:209-213 and 344-347 are backed.

- **First-terminal-wins arbitration and the benign revoke race remain coherent
  under the two-task model.** The two spool sites (`supervisor.rs:627/661`) set
  `spool_exhausted` (`:616/:659`) and an authoritative `Failure`
  `pending_terminal` (`:633/:666`) **before** firing the token; the guard at
  `:587-590` drops the later token-driven `Cancelled`, so the spool `Failure`
  stays authoritative (011:233-242). The operator-revoke race is documented as
  benign and matches today (011:243-251). The two-task model does not disturb
  either: the completion task is the sole terminal producer, and it checks
  `spool_exhausted`/`pending_terminal` and suppresses (011:240-242).

## Findings (ordered by severity)

No blocker. Two nits.

### N4 — nit (new): the cancel token's clone/plumbing is unspecified (confidence 82)

The single `CancellationToken` has two consumers that must share cancel state:
the record (so the four `token.cancel()` sites — `on_build_revoke`, `shutdown`,
the two spool teardowns — can fire it) and the build task's `RunOpts` (so
`runner::run`'s `select!` observes it). The doc says the token goes into
`RunOpts` (011:89) and that `register_accepted` "takes the token" (011:180,
223), but it never states the token is **cloned**, nor the create-once order.
`tokio_util::sync::CancellationToken` is `Clone` and a clone shares the same
cancellation state, so the correct plumbing is: the handler creates one token,
clones it into the build task's `RunOpts` at spawn, and passes the token (or
another clone) into `register_accepted` for the record. As written, a careless
reader could move the token into `RunOpts` and leave the record with nothing to
cancel, or vice versa. This is a one-sentence plumbing clarification, not a
design defect — the mechanism (009's `select!`-on-token) is correct and
unchanged.

**Recommended.** State that a single token is created at dispatch, **cloned**
into `RunOpts` at build-task spawn, and stored in the record at register so all
four cancel sites and the runner observe the same cancellation.

### N5 — nit (new): the "exactly as today's `retire()` tolerates a `None` `output_task`" parity sentence glosses a benign register-window asymmetry (confidence 80)

011:190-192 says the handle "lives in an `Option`, `.take()`-d for a single
await, so `retire()` and `shutdown()` cannot both await it (and either tolerates
a not-yet-attached `None` by skipping the await, exactly as today's `retire()`
tolerates a `None` `output_task`)." The `Option`/`.take()` parity is accurate,
but the "exactly as today" framing understates one real difference in the
register→attach window. **Today** the executor — which is `shutdown()`'s
teardown handle (`shutdown()` `exec.kill()` at `:503`, `exec.wait()` at `:517`)
— is stored **at register time** (`register_accepted` takes the executor,
`:173/:186`), so `shutdown()`'s teardown barrier holds throughout the window.
**Under v4** the teardown handle (the completion-task handle) arrives only at
`attach_completion_task`, so for the brief register→attach window the await
barrier is genuinely absent; teardown in that window rests instead on
`token.cancel()` (which the doc fires before the skipped await, 011:209) plus
the fact that no container can be up yet.

This is **benign**, and I verified why:

- `shutdown()` has exactly one caller (`handler.rs:247`), the
  `state.notify.notified()` branch of `run_connection`'s `select!`. The
  `BuildNew` path that runs `handle_build_new` (register + spawn + attach, all
  to completion) is the sibling `receiver.next()` branch (`handler.rs:111`). A
  `select!` runs one branch to completion before re-polling, so `shutdown()`
  cannot interleave **between** `register_accepted` and `attach_completion_task`
  within a single dispatch. The window is not concurrently reachable today.
- Even if it were, `token.cancel()` is delivered regardless of the `None` handle
  (only the await is skipped), and `runner::run` performs config-load +
  component aggregation + secrets marshalling + config rewrite (009:45-64)
  before `podman_run` — so no builder container exists during the
  register→attach window for the missing barrier to leak.

So there is no teardown leak. The fix is to soften the parity claim so it does
not imply the barrier coverage is identical to today's in that window.

**Recommended.** Note that the completion-task handle (the teardown barrier)
arrives at attach rather than register, and that the register→attach window is
covered by `token.cancel()` firing before the await plus the serialization of
the dispatch and shutdown `select!` branches — rather than asserting it is
"exactly as today."

## Confidence score

| Item                                                                              | Points | Description                                                                                                                                   |
| --------------------------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                    | 100    |                                                                                                                                               |
| N4 (D11): cancel-token clone/plumbing unspecified                                 | -3     | Token shared by record + `RunOpts`; doc never says "cloned" nor the create-once order — one-sentence gap                                      |
| N5 (D11): "exactly as today" parity glosses the register-window barrier asymmetry | -3     | Teardown handle now arrives at attach, not register; benign (branch serialization + early `token.cancel()`) but the wording overstates parity |
| **Total**                                                                         | **94** |                                                                                                                                               |

Interpretation: 94 — **ready to merge; minor or no issues.** The score rose from
v3's 74 because the sole v3 blocker (F1-c) and both v3 nits (N2, N3) are
resolved, and the new-gap hunt produced only two exposition nits, no blocker.

## Recommended actions (non-blocking)

1. **N4** — Add one sentence: a single `CancellationToken` is created at
   dispatch, cloned into the build task's `RunOpts`, and stored in the record so
   all four cancel sites and `runner::run` share cancel state.
2. **N5** — Soften 011:190-192 so it does not claim the register→attach window
   is "exactly as today"; note the teardown handle arrives at attach and the
   window is covered by the early `token.cancel()` plus `select!`-branch
   serialization.

## Resolution status

- **F1-c (v3 BLOCKER): RESOLVED.** The doc specifies the two-step
  `register_accepted(build_id, token, component_dir)` → spawn build + completion
  tasks → `attach_completion_task(build_id, completion_handle)` ordering, which
  matches today's proven `register_accepted`→`attach_output_task` shape
  (`handler.rs:491-496`/`:519-525`; `supervisor.rs:170-194`/`:198-205`). The
  record provably exists before the completion task can emit, so a fast-failing
  build's terminal is not orphan-dropped at `supervisor.rs:270-274`. The
  one-step form and its self-contradicting "mirrors today's `output_task`
  pattern" justification are gone; `retire()`/`shutdown()` tolerate a
  not-yet-attached `None` handle by skipping the await (`supervisor.rs:451/459`,
  `:508/514`).
- **N2 (v3 nit): RESOLVED.** 011:60-61 now states the model as **two tasks** (a
  build task and a completion task), not "one supervised build task."
- **N3 (v3 nit): RESOLVED.** 011:205-209 now describes the `retire()` await as
  "brief and bounded, not instant," with the correct mpsc-enqueue-then-return
  reasoning.
- **No new blocker.** The register→attach window introduced by the v4 patch is
  benign (not concurrently reachable by `shutdown()`; `token.cancel()` fires
  before the skipped await; no container is up before attach). The two remaining
  items (N4, N5) are exposition nits, not a fresh correctness defect and not a
  structural problem with the completion-task / register-attach seam.
