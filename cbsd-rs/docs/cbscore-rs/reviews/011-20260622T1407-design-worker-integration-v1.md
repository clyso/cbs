# 011 — Worker integration (in-process cutover) — design review v1

Adversarial review of
`cbsd-rs/docs/cbscore-rs/design/011-20260622T1359-worker-integration.md` (the
final design of the cbscore→Rust port set: the worker's Python-subprocess →
in-process `cbscore::runner::run` cutover).

Method: every claim was traced into the actual worker source
(`cbsd-worker/src/build/{executor,output,supervisor,component}.rs`,
`ws/handler.rs`), the retired contract (`scripts/cbscore-wrapper.py`), the
contracts it builds on (009, 006, 004, 001), and the wire type
(`cbsd-proto/src/ws.rs`). The implementer is not trusted; line references below
are first-hand.

## Verdict

**NO-GO as written / conditional.** The descriptor-translation, native-result,
cancellation-vs-timeout-ownership, and run-name claims are accurate and
faithfully reproduce the wrapper and 009. But the document's single most
important contribution — the **B2 panic-isolation seam** that is a named
acceptance gate (001 invariant 6) — is described in a way that is internally
contradictory and, under the wiring the document itself sketches, **does not
reach the failure path it claims**. A panicking build task would wedge the
supervisor in "busy" rather than surfacing a `Failure` and continuing to serve.
That is the whole point of the review, and it must be resolved before
implementation. Three further findings (the incomplete kill→cancel migration,
the disappearing 7200 s timeout default, the dropped 64 KB report cap) are real
parity/coupling gaps. Fix F1 and F2 and re-review; F3–F5 are accept-or-amend.

## What the document gets right (verified)

- **Cutover removal table (claim 1).** `executor.rs` really does `setsid` in
  `pre_exec` (156–163), SIGTERM→SIGKILL escalation with
  `DEFAULT_SIGKILL_TIMEOUT_SECS` (30, 215–251), and
  `classify_exit_code(137|143 → Revoked)` (275–281). `output.rs` really parses
  the `{"type":"result"}` sentinel (107–164) and has the "no result line ⇒
  `Failure`" fallback (215–224). Removing all of this is correct in principle.
- **Descriptor translation (claim 2)** mirrors the wrapper exactly:
  `component_refs` name→ref (wrapper 170–173), `component_uri_overrides` from
  `repo` where set (175–179), `el_version` from `elN` (58–63, 156), the registry
  guard (148–149), the non-empty `dst_image.tag` guard (126–128), `user`/`email`
  from `signed_off_by` (185–186), and the `components` path override (152).
  `version_create_helper` is genuinely owned by 006.
- **Run name (claim 3).** The wrapper uses `cbs-{trace_id.replace('-','')[:12]}`
  with `replace_run=True` (234, 250). The doc's
  `"cbs-" + trace_id.replace('-',"")[..12]` + `replace_if_exists = true`
  matches, and 009 explicitly assigns the deterministic trace-derived run name
  to the worker caller (009 "Run name & two callers").
- **Native result mapping (claim 6).** 009 defines `RunnerError` with
  `NonZeroExit { report, stderr }` (the only report-bearing variant), `Podman`,
  and `Cancelled`. The wire type confirms the doc's claim that a report may ride
  any status: `WorkerMessage::BuildFinished` (ws.rs:167–174) carries `status`,
  `error`, and `build_report: Option<serde_json::Value>` as independent fields.
  Today's `build_report: None` on synthetic failures (supervisor.rs:633, 666;
  handler.rs:480, 506, 584, 597) is a **code choice, not a type constraint** —
  exactly as the doc states.
- **`classify_exit_code` criticism (claim 7).** The 137/143⇒Revoked heuristic
  exists (executor.rs:275–281) and genuinely cannot distinguish an operator
  revoke from an unrelated SIGKILL/OOM; `RunnerError::Cancelled` is a strictly
  better signal.
- **Cancellation vs timeout ownership (claim 5, mechanism).** 009 defines
  `RunnerError::Cancelled`, the `select!`-on-token +
  `podman_stop(name = ctr_name)` mechanism, and keeps the build timeout inside
  `podman_run`. The doc's description of the mechanism is faithful to 009.
- **No `panic = "abort"` in the workspace.** Confirmed: no `[profile…]` section
  exists in any of the six `cbsd-rs` `Cargo.toml` files. So
  `JoinError:: is_panic()` will fire (the default `unwind` holds). The doc's
  premise is true — but see F2 for what it omits.

## Findings (ordered by severity)

### F1 — BLOCKER: the panic-isolation seam is contradictory and, as wired, unreachable (confidence 90)

The doc describes **two** consumers awaiting **one** `JoinHandle`:

1. an always-on "join-watcher [that] awaits the build task's `JoinHandle`" and,
   on `Err(e).is_panic()`, synthesizes a `Failure` terminal (lines 147–153); and
2. `retire()`/`shutdown()`, where "the `JoinHandle` replaces the old
   `output_task` handle" (lines 142, 230) — i.e. the supervisor awaits the same
   handle.

A `tokio::JoinHandle` can be awaited exactly once. These two descriptions cannot
both hold for the same handle, and the doc never says which one owns the await.
Tracing the real code shows the contradiction is not academic:

- `retire()` (supervisor.rs:439) is the only normal-operation site that awaits
  `output_task` (451, 459–462). It is called **only after a `BuildFinished` is
  observed on the outbound channel** (handler.rs:98–104 in the reconnect drain,
  and 217–233 in the main loop).
- A **panicking** build task sends **no** `BuildFinished`. Therefore `retire()`
  is never called, the handle is never awaited, `is_panic()` is never observed,
  and `state.active` is **never cleared**.
- The next `BuildNew` then hits `handle_build_new`'s busy guard (handler.rs:390)
  and is rejected as "worker is busy" — **forever**. This directly falsifies the
  doc's claim that "the worker process is unaffected and continues serving"
  (line 153) and the acceptance gate "the worker keeps serving subsequent
  `BuildNew`s" (line 239).

For the seam to work, the watcher must **own** the single await and _drive_
retirement on a panic (clear `active`, clean up the component dir, emit the
synthetic terminal), rather than retirement owning the await. That is a real
change to the supervisor's ownership model — which **contradicts the "Supervisor
unchanged" / "only `executor` → `cancel_token + JoinHandle`" claim** (lines
228–230). The substitution is not field-for-field; it reshapes who awaits what
and when `active` is cleared.

Compounding the overclaim: lines 151–152 and 218–219 say the panic maps to a
`Failure` "via the **existing** 'no-terminal ⇒ Failure' path." That path lives
in `output.rs:215–224`, which this very cutover **deletes** ("Removed:
`output.rs`'s … parsing and `ChildStdout` reading", lines 50–53).
`on_output_message` (supervisor.rs:254) has no such synthesis. The panic→Failure
synthesis is **entirely new code**, not reuse of an existing path.

**Required:** specify the single owner of the `JoinHandle` await; specify that
on `is_panic()` (and on a non-panic `Err`/early `Ok` with no terminal seen) the
owner clears `active`, runs `component::cleanup`, and emits a synthetic
`Failure` terminal through `on_output_message`; and stop describing this as
"Supervisor unchanged" and as reuse of the deleted output.rs fallback.

### F2 — BLOCKER (parity with 001): `panic = "unwind"` is asserted but never pinned (confidence 88)

001 is explicit (B2 + correctness invariant 6): **"The worker release profile
must pin `panic = "unwind"` explicitly, so no future profile edit can switch it
to `abort` and silently void this invariant. (The workspace sets no `panic`
value today, so `unwind` already applies by default; the point is to pin it, not
to fix a flip.)"**

The doc instead says (lines 144–146, 217–219): "The worker keeps
`panic = "unwind"` — verified: the workspace sets no `panic = "abort"` profile,
so the default holds." That is **strictly weaker** than 001 requires: it
documents the current default but does **not** add the pin. Verified: no
`[profile.release]` (or any `[profile…]`) block exists in `cbsd-rs/Cargo.toml`
or any member manifest, so the explicit pin 001 mandates is absent and the doc
does not call for adding it. If a later edit adds `panic = "abort"` (a common
release-size optimization), `is_panic()` never fires — the process aborts — and
the entire seam is silently void, exactly the failure mode 001 wrote the pin to
prevent.

**Required:** the design must mandate adding
`[profile.release] panic = "unwind"` (workspace root or worker) as 001
specifies, not merely observe the default.

### F3 — incomplete kill→cancel migration; "Supervisor unchanged" hides two coupled call sites (confidence 85)

The cancellation section (lines 166–167) names only two `exec.kill()` callers —
`on_build_revoke` and `shutdown`. There are **four** in the real supervisor:

- `on_build_revoke` (supervisor.rs:235),
- `shutdown` (503),
- **spool-overflow** (627), and
- **spool-write-error** (661).

The latter two live inside the spool machinery the doc repeatedly calls "kept
unchanged" / "orthogonal to the execution mechanism" (lines 42–47, 228–230).
Replacing the `executor` field with a `CancellationToken` forces **all four**
sites to migrate to `token.cancel()`, so the spool path is **not** untouched.

There is also a behavioural subtlety the doc must address. The two spool sites
today synthesize a **`Failure`** terminal (633, 666) after killing. If they
instead fire the token, the build task returns `RunnerError::Cancelled` →
**`Revoked`** under the doc's mapping (line 114) — a different terminal than the
"spool exceeded"/"spool write error" `Failure` the operator should see. The
existing `spool_exhausted` guard (587–590) suppresses any subsequent terminal
from `on_output_message`, so the synthetic `Failure` should still win the race —
but the design must **state** this explicitly rather than imply the spool path
is unaffected, and confirm the token-driven `Cancelled` terminal is dropped by
that guard.

### F4 — the 7200 s build-timeout default disappears (confidence 84)

The 7200 s (2 h) default lives in the **wrapper**
(`int(os.environ.get( "CBS_BUILD_TIMEOUT", "7200")`, wrapper:233), which the
cutover deletes. The worker only sets `CBS_BUILD_TIMEOUT` when
`config.build_timeout_secs` is `Some` (executor.rs:150–152); when unset, the
build relies on the wrapper's 7200 s fallback. The doc says `RunOpts.timeout`
comes "from its own configured build timeout (the wrapper's `CBS_BUILD_TIMEOUT`,
default 7200 s)" and asserts "009's 4 h default applies only to the CLI path
that passes no override" (lines 222–224). That is **wrong for the unconfigured
worker**: with the wrapper gone and no worker default, an unconfigured worker
passes no override and inherits 009's **4 h** runner default — a silent doubling
of the build deadline.

**Required:** either move the 7200 s default into the worker (preserving
parity), or explicitly accept the change to 4 h. The doc currently does neither;
it states a default it no longer enforces.

### F5 — the 64 KB `build_report` size cap silently drops (confidence 82)

`output.rs` enforces `MAX_REPORT_SIZE = 65_536` and discards an over-cap report
with a warning (33–35, 115–128). The native-result path bypasses `output.rs`
entirely (the doc deletes it), so this bound disappears. A pathologically large
`NonZeroExit.report` or success report would now be forwarded verbatim in
`BuildFinished.build_report`, with no worker-side ceiling. The doc does not
mention whether any bound is re-imposed (in the worker, or upstream in 009's
report read).

**Recommended:** state explicitly whether the 64 KB cap is intentionally dropped
(and why that is safe given the server-side handling) or re-imposed in the
worker's result-mapping step.

### N1 — exposition: data-dependency order inverted (nit, confidence 70)

The in-process-task list presents descriptor translation (step 1) before config
load (step 2), yet step 1's `registry` (`config.storage.registry.url`) and
`components_paths` (`config.paths.components`) are **produced by** step 2's
`Config::load`. The wrapper does config-load first (130–152), then
`version_create_helper` (167). Reorder the steps to match the real data
dependency, or note that translation consumes the loaded config. Cosmetic; no
behavioural impact.

## Confidence score

| Item                                                                | Points | Description                                                                                                |
| ------------------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------- |
| Starting score                                                      | 100    |                                                                                                            |
| F1 (D8): panic seam contradicts its own JoinHandle ownership        | -20    | Single handle, two awaiters; panic path unreachable as wired — breaks a named acceptance gate              |
| F1 (D8): "existing no-terminal ⇒ Failure path" reuse claim is false | -5     | That path is in deleted `output.rs`; synthesis is new code                                                 |
| F2 (D8): `panic = "unwind"` not pinned as 001 mandates              | -15    | Design asserts the default instead of requiring the explicit pin 001/invariant 6 demand                    |
| F3 (D8): kill→cancel migration omits two spool call sites           | -10    | "Supervisor unchanged" hides coupling at supervisor.rs:627/661; `Cancelled`→`Revoked` vs synthetic Failure |
| F4 (D8): 7200 s timeout default silently becomes 009's 4 h          | -10    | Wrapper-owned default deleted; unconfigured worker inherits the runner's 4 h                               |
| F5 (D9): 64 KB `build_report` cap dropped without mention           | -5     | `MAX_REPORT_SIZE` bypassed by the native path; no replacement stated                                       |
| N1 (D11): step order inverts the config→translate data dependency   | -3     | Exposition nit                                                                                             |
| **Total**                                                           | **32** |                                                                                                            |

Interpretation: 0–49 — **major rework needed; block.** The score is dominated by
the panic-seam contradiction (F1) and the missing pin (F2), both of which sit on
the B2 acceptance gate that is the document's reason to exist.

## Required actions before GO

1. **F1** — Name the single owner of the build-task `JoinHandle` await and
   specify the panic/early-exit path that clears `active`, cleans the component
   dir, and emits a synthetic `Failure` terminal. Drop the "Supervisor
   unchanged" and "existing no-terminal ⇒ Failure path" claims (the supervisor
   ownership changes; the synthesis is new).
2. **F2** — Mandate pinning `panic = "unwind"` in a release profile, per 001
   invariant 6 / B2.
3. **F3** — Migrate all four `exec.kill()` sites to the token; document the
   spool sites and confirm the `spool_exhausted` guard keeps the synthetic
   `Failure` terminal authoritative over a token-driven `Cancelled`.
4. **F4** — Move the 7200 s default into the worker, or explicitly accept the 4
   h runner default for an unconfigured worker.
5. **F5** — State whether the 64 KB report cap is intentionally dropped or
   re-imposed.
