# 011 — Worker integration (in-process cutover) — design review v3

Third-round adversarial review of
`cbsd-rs/docs/cbscore-rs/design/011-20260622T1359-worker-integration.md` (the
final design of the cbscore→Rust port set: the worker's Python-subprocess →
in-process `cbscore::runner::run` cutover). Round v1 returned NO-GO / 32-100
(blockers F1, F2; majors F3–F5; nit N1). Round v2 confirmed F2–F5 + N1 resolved
and the F1 _wedge_ fixed, but raised F1-b (BLOCKER: build-task `JoinHandle`
ownership self-contradictory; `shutdown()` teardown await unbacked) and F6
(minor: revoke/completion race). This round verifies the v2 fixes against the
live worker source and re-hunts for defects the rework introduced — with
specific attention to the completion-task/handle-ownership model and the
spawn/registration wiring.

Method: every claim was re-traced into the live worker source
(`cbsd-worker/src/build/{executor,output,supervisor,component}.rs`,
`ws/handler.rs`), the retired contract (`scripts/cbscore-wrapper.py`), the
contracts it builds on (009, 006, 004, 001), and the workspace manifests. The
implementer is not trusted; line references below are first-hand.

## Verdict

**NO-GO as written / conditional.** F1-b is genuinely resolved: the doc now
specifies a concrete two-task model (build task + completion task) with a
single, consistent owner of each handle, and — verified against the real
`retire()`/`shutdown()`/`on_output_message` code — the model is sound. The
completion task is the sole owner of the build-task handle; the `ActiveBuild`
record stores the **completion-task** handle in an `Option`, `.take()`-d for a
single await; `shutdown()` awaiting that handle is a real teardown barrier (the
completion task does not return until the build task returns, which after a
cancel is after `runner::run`'s `podman_stop`). The `.take()` race between
`retire()` and `shutdown()` is safe in every ordering. F6 is now documented as
an intentional, benign race and the "matches today's behaviour" claim checks
out.

But the rework **introduced a new correctness defect on the same seam** (F1-c):
collapsing the old two-step `register_accepted` → `attach_output_task` into a
**single** `register_accepted(token, completion_handle)` forces the completion
task to be **spawned before** the active record exists. A fast-failing build
task can then emit its terminal through `on_output_message` **before**
`register_accepted` has set `state.active`, hitting the "no active build → drop
orphan" path (`supervisor.rs:270-274`): the `BuildFinished` is silently dropped
and the build hangs server-side. This is the precise race today's code is wired
to avoid, and the doc even contradicts itself by calling the one-step form
"mirroring today's `output_task` pattern" (which is two-step). Because it sits
on terminal delivery for the fast-failure paths the doc itself introduces
(translation guards, config-load failure), it must be fixed before
implementation. This is again the "each fix spawns the next round's finding"
pattern on the panic/cancellation seam. Two cosmetic items (N2 stale "one build
task" singular; N3 imprecise "the await is immediate") do not block.

Fix F1-c and re-review; N2/N3 are accept-or-amend.

## Status of each prior finding

| Finding                                       | Status            | Evidence                                                                                                                                                          |
| --------------------------------------------- | ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F1-b — completion-handle ownership / teardown | **Resolved**      | Two-task model now concrete (011:155-200); record stores the completion-task handle, `.take()`-d once; `shutdown()` barrier verified sound against supervisor.rs. |
| F6 — revoke/completion terminal race          | **Resolved**      | Documented as intentional/benign (011:228-236); "matches today" verified (streamer `Success` still flows via `on_output_message` after revoke clears pending).    |
| F2 — `panic = "unwind"` not pinned            | **Resolved (v2)** | 011:185-189 mandates pinning in workspace `[profile.release]`; no `[profile]` block in any manifest (re-verified).                                                |
| F3 — kill→cancel omits two spool sites        | **Resolved (v2)** | All four sites named (011:213-215 → supervisor.rs :235/:503/:627/:661); first-terminal-wins arbitration sound against :587-590/:616/:659.                         |
| F4 — 7200 s timeout default disappears        | **Resolved (v2)** | Worker injects 7200 s when unset (011:298-302); matches wrapper:233.                                                                                              |
| F5 — 64 KiB report cap dropped                | **Resolved (v2)** | Cap re-homed to completion path (011:137-141, 283-284); output.rs:35 = 65 536.                                                                                    |
| N1 — config/translate step order inverted     | **Resolved (v2)** | Config-load is step 1; dependency stated (011:63-67).                                                                                                             |

## What the rework gets right (verified this round)

- **F1-b — the completion-task/handle model is now concrete and sound.** The doc
  specifies (011:155-200): a `tokio::spawn`'d **build task** that returns the
  typed `Result` and emits no terminal; a `tokio::spawn`'d **completion task**
  that is the **sole owner** of the build-task `JoinHandle`, awaits it once,
  maps the outcome (or synthesizes a `Failure` on `is_panic()`), and emits the
  terminal via `on_output_message`. The `ActiveBuild` record stores the
  **completion-task** handle (not the build-task handle), held in an `Option`
  and `.take()`-d so only one of `retire()`/`shutdown()` awaits it
  (011:174-183). Every handle-ownership statement in the doc is now mutually
  consistent — 011:159-160, 174-183, 192-200, 211-212, 285-291 all say the same
  thing (record = completion-task handle; build-task handle = completion task's
  alone). The v2 contradiction (record stores build-task handle vs. completion
  path owns it) is gone.

- **The teardown barrier is real.** I traced the claim against the live code.
  `shutdown()` (supervisor.rs:496-539) is the worker's only clean stop path;
  today it cancels, awaits the streaming task, then `exec.wait()`s the child.
  Under v3, `shutdown()` cancels the token and `.take()`s/awaits the
  completion-task handle. The completion task awaits the **build task**, which
  (per 009:154-167) returns only after `runner::run` has done its cancel-path
  `podman_stop(name = ctr_name)`. So the chain
  `shutdown().await → completion task → build task → podman_stop` guarantees the
  container is stopped before the worker exits — the in-process analog of
  today's `child.wait()`. The doc's claim (011:196-200, 329-332) is backed.

- **No self-join in the `retire()` path.** `retire()` is spawned by the handler
  as its own task when the terminal is observed on the outbound channel
  (handler.rs:225-233: `tokio::spawn(async move { sup.retire(bid)... })`), which
  is a **different** task from the completion task. `retire()` awaits the
  completion-task handle; the completion task does not await its own handle. No
  self-join, no deadlock. (See N3 for the cosmetic imprecision in calling this
  await "immediate".)

- **The `.take()` race between `retire()` and `shutdown()` is safe in every
  ordering.** Both `.take()` the completion-task handle under the state lock, so
  exactly one awaits it; the loser awaits nothing. Either outcome is sound: in
  the `retire()`-first case, the terminal was already emitted, which means the
  completion task already mapped the result, which means the build task already
  returned (`runner::run` done, container already stopped) — so a later
  `shutdown()` finding `None` (or `active` already dropped) awaits nothing but
  the container is already gone. In the `shutdown()`-first case, `shutdown()`
  awaits the completion task → build task → `podman_stop`. No ordering misses
  teardown or cleanup.

- **F6 — revoke/completion race documented as benign, and "matches today" is
  accurate.** 011:228-236 states `BuildRevoke ⇒ Revoked` is not strictly
  guaranteed: a natural `Ok`/`NonZeroExit` return in the cancel window reports
  the real terminal, and this is intentional and matches today. Verified:
  `on_build_revoke` (supervisor.rs:224-244) transitions to `Revoking` and clears
  `pending_terminal`, but the streaming task's own `BuildFinished(Success)`
  still flows through `on_output_message` if the subprocess had already exited 0
  — so today a revoke racing a real completion likewise reports the true
  outcome. The doc no longer implies determinism it cannot establish.

- **F2–F5 + N1 remain resolved.** Re-verified the v2 evidence: the
  `panic = "unwind"` pin is mandated and homed at the workspace
  `[profile.release]` (no `[profile]` block exists in `cbsd-rs/Cargo.toml` or
  any member manifest); all four `exec.kill()` sites
  (supervisor.rs:235/503/627/661) map to `token.cancel()` with sound
  first-terminal-wins arbitration (the spool sites set `spool_exhausted` at
  :616/:659 and an authoritative `Failure` `pending_terminal` at :633/:666
  before cancelling, and the :587-590 guard drops the later token-driven
  `Cancelled`); the 7200 s timeout default is re-homed into the worker
  (wrapper:233 parity); the 64 KiB report cap is re-homed to the completion path
  (output.rs:35). N1's config-before-translate order is fixed.

- **Descriptor-translation, native-result, run-name, crash-containment
  mechanism, and `component.rs` retention** remain accurate (re-confirmed
  against wrapper :58-63, :127-128, :148-149, :152, :167-187, :233-234, 009's
  `RunnerError`/`podman_stop` contract, and component.rs
  `validate_and_unpack`/`cleanup`). These were correct in v1/v2 and are
  unchanged.

## Findings (ordered by severity)

### F1-c — BLOCKER (new): single-step `register_accepted(token, completion_handle)` forces spawn-before-register, reopening the orphan-drop window (confidence 88)

The v3 model has `register_accepted` take the **completion-task** handle:
"`register_accepted` takes `(token, completion_handle)`" (011:176-178), and
"`register_accepted` takes the token and the **completion-task** `JoinHandle`"
(011:211-212). To pass that handle, the completion task must be **spawned
before** `register_accepted` runs. But the completion task's whole job is to
await the build task and then emit the terminal via `on_output_message`
(011:159-165). So the ordering the doc specifies is:

1. spawn build task,
2. spawn completion task (awaits build task → will call `on_output_message`),
3. `register_accepted(token, completion_handle)` → sets `state.active`.

On the multi-thread tokio runtime 001 mandates, steps 2 and 3 race. A
**fast-failing build task** returns almost immediately, the completion task maps
it and calls `on_output_message(BuildFinished{..})` — and if that lands before
step 3, `on_output_message` hits the guard at supervisor.rs:270-274:

```rust
let Some(active) = state.active.as_mut() else {
    tracing::debug!(%build_id, "dropping orphan output message");
    return;
};
```

The terminal is **silently dropped**. The server never receives a
`BuildFinished`, the build hangs in a non-terminal state server-side, and
(because `retire()` only fires on an observed terminal) `active` is never set to
begin with — but the build outcome is lost. Fast-failure is not hypothetical:
the doc's own build-task steps fail fast on a bad `os_version`, missing
registry, or empty `dst_image.tag` (011:74-79) and on `Config::load` of a
missing/invalid config (011:63-67) — all before any container is spawned.

This is exactly the race the **current** code is wired to avoid. handler.rs
deliberately uses a **two-step** sequence:

- handler.rs:491-496 — `register_accepted(build_id, exec, component_dir)`
  **first**, with an explicit comment: _"Register the active build BEFORE
  spawning the streaming task so the supervisor sees the executor in case the
  streamer produces a message immediately."_
- handler.rs:519-525 — spawn the producer task, then store its handle via a
  **separate** call `attach_output_task(build_id, task)`
  (supervisor.rs:198-205), which fills the `output_task` slot on the
  already-present record.

v3 collapses these two steps into one, which forces the producer (the completion
task) to exist before the record — the opposite of the invariant the comment
protects.

The doc also **contradicts itself** here: 011:178-180 says the record holds the
completion handle "in an `Option` and `.take()`s it for a single await
(**mirroring today's `output_task` pattern**)". Today's `output_task` pattern is
precisely the two-step register-then-attach sequence — the single-step
`register_accepted(token, completion_handle)` two lines earlier is **not**
mirroring it; it is the inversion that reopens the window.

**Required.** Preserve the two-step ordering: `register_accepted` sets
`state.active` with the **token only** (no handle) first; then spawn the build
and completion tasks; then attach the completion-task handle via a separate call
into the `output_task` slot (the `attach_output_task` analog). Spelled that way
the orphan-drop window closes (the record exists before the completion task can
emit), and the "mirroring today's `output_task` pattern" claim becomes true. As
written, the design specifies the racy one-step ordering and the
self-contradicting "mirrors today" justification.

### N2 — nit (new): stale "one supervised build task" singular contradicts the two-task model (confidence 82)

The section header is "## The in-process build task" (011:56) and 011:60-61 says
the executor-spawn + output-streamer pair "is replaced by **one supervised build
task** per build." That singular framing predates the v3 rework, which is
explicitly **two** tasks (build task + completion task, 011:155-160). A reader
who stops at the overview takes away the wrong owner model. Reword 011:56/60-61
to reflect the build-task + completion-task pair (or note that the "build task"
of the overview is elaborated into two tasks in the Crash-containment section).
Exposition only; no behavioural impact.

### N3 — nit (new): "the await is immediate" overstates the retire ordering (confidence 80)

011:192-196 says `retire()` "`.take()`s and awaits the stored
**completion-task** handle — which has already finished by the time the terminal
it emitted is observed, so the await is immediate." Tracing the sequence: the
completion task calls `on_output_message`, whose `t.outbound.send(msg).await`
(supervisor.rs:291) returns once the message is enqueued in the bounded mpsc
(capacity 64, handler.rs:91) — **not** when the handler receives it. The
completion task then returns. So when the handler `recv()`s the terminal
(handler.rs:217) and spawns `retire()`, the completion task may still be
finishing (between `send().await` returning and its closure returning). The
await is **bounded and short**, not strictly "immediate / already finished."
This is harmless (the await simply waits the residual), but the wording asserts
an ordering guarantee that does not hold. Soften to "bounded — the completion
task is finishing once its terminal is observed."

## Confidence score

| Item                                                                                  | Points | Description                                                                                                                            |
| ------------------------------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                        | 100    |                                                                                                                                        |
| F1-c (D8): one-step `register_accepted(token, completion_handle)` reopens orphan-drop | -15    | Forces spawn-before-register; fast-failing terminal drops at supervisor.rs:270-274; build hangs server-side — terminal-delivery defect |
| F1-c (D8): "mirroring today's `output_task` pattern" is false                         | -5     | Today's pattern is two-step register-then-attach; the specified one-step form is its inversion                                         |
| N2 (D11): stale "one supervised build task" singular vs two-task model                | -3     | Overview contradicts the corrected ownership model; misleads a reader who stops there                                                  |
| N3 (D11): "the await is immediate" overstates the retire ordering                     | -3     | `send().await` returns on enqueue, not receipt; completion task is finishing, not finished, when the terminal is observed              |
| **Total**                                                                             | **74** |                                                                                                                                        |

Interpretation: 74 — **significant issues; must address before proceeding.** The
score is dominated by F1-c, the new terminal-delivery race on the registration
seam. The v2 blocker (F1-b) and all v1 findings are resolved, which is why the
score holds near the v2 level despite the new finding; F1-c is narrower than
F1-b (a fixable ordering rule, not a self-contradictory ownership model) but
lands on the same correctness-critical seam.

## Required actions before GO

1. **F1-c** — Specify the two-step registration ordering: `register_accepted`
   sets `state.active` with the cancel token only; spawn the build and
   completion tasks **after** the record exists; attach the completion-task
   handle via a separate call into the `output_task` slot. This closes the
   orphan-drop window for fast-failing builds and makes the "mirroring today's
   `output_task` pattern" claim accurate. Reconcile 011:176-180 and 011:211-212
   with this two-step sequence.
2. **N2** — Reword the overview (011:56, 60-61) so "one supervised build task"
   reflects the build-task + completion-task pair the rework actually
   introduces.
3. **N3** — Soften "the await is immediate" (011:195-196) to "bounded — the
   completion task is finishing once its terminal is observed," matching the
   `send().await`-on-enqueue semantics.

## Resolution status

- **F1-b (v2 BLOCKER): RESOLVED.** The completion-task/handle-ownership model is
  now concrete and internally consistent; the completion task is the sole owner
  of the build-task handle; the record stores the completion-task handle
  (`.take()`-d once); `shutdown()` awaiting it is a verified teardown barrier
  (completion task → build task → `podman_stop` → container stopped before
  exit); the `retire()`/`shutdown()` `.take()` race is safe in every ordering;
  no self-join.
- **F6 (v2 minor): RESOLVED.** The revoke/completion race is documented as
  intentional and benign, and the "matches today's behaviour" characterization
  is accurate against the live streamer/`on_build_revoke` code.
- **F1-c (NEW BLOCKER):** the rework's single-step
  `register_accepted(token, completion_handle)` forces spawn-before-register and
  reopens the orphan-drop window — must be fixed (two-step ordering) before GO.
