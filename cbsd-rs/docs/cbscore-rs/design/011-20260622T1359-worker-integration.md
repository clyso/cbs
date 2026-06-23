# 011 — Worker integration (in-process cutover)

This is the reference design for the **cutover**: replacing the `cbsd-worker`'s
Python-subprocess build path (`cbsd-rs/scripts/cbscore-wrapper.py`, launched via
`python3` and driven over stdin/stdout) with a **direct in-process call to
`cbscore::runner::run`** (009). It is the whole point of the port (001): the
worker links the Rust `cbscore` library and consumes `runner::run`'s
`Result`/report natively instead of parsing a subprocess's exit code and stdout.

Source of truth: the current worker build path —
`cbsd-rs/cbsd-worker/src/build/{executor,output,supervisor,component}.rs` and
`ws/handler.rs` (the `BuildNew` dispatch) — and the contract being retired,
`cbsd-rs/scripts/cbscore-wrapper.py`. The runner it now calls is 009; the
`VersionDescriptor` construction it absorbs is 006.

Build isolation is **unchanged**: the actual build still runs inside a podman
builder container. What changes is who orchestrates it — an in-process
`runner::run` task instead of a `python3` subprocess running the same
`runner()`.

## What the cutover moves, keeps, and removes

The wrapper script does **two** jobs; the cutover absorbs both into the worker
process:

1. **Translate** the `cbsd_proto::BuildDescriptor` (the WS wire type the worker
   receives) into a cbscore `VersionDescriptor` via `version_create_helper`
   (`cbscore-wrapper.py:167-187`), writing it to a temp file.
2. **Run** `cbscore.runner.runner(...)` and report the outcome
   (`cbscore-wrapper.py:243-262`).

| Concern                          | Today (subprocess)                                  | After cutover (in-process)                                         |
| -------------------------------- | --------------------------------------------------- | ------------------------------------------------------------------ |
| Descriptor → `VersionDescriptor` | wrapper, in Python                                  | worker, in Rust, via `version_create_helper` (006)                 |
| Build orchestration              | `python3 cbscore-wrapper.py` → `runner()`           | `cbscore::runner::run` (009) on a worker task                      |
| Result/report transfer           | parse `{"type":"result"}` stdout line + exit code   | native `Result<Option<BuildArtifactReport>, RunnerError>`          |
| Log streaming                    | read `ChildStdout` line-by-line                     | `runner::run`'s streaming callback (009 `RunOpts` log sink)        |
| Cancellation                     | SIGTERM→SIGKILL to the subprocess **process group** | `CancellationToken` → 009 `podman_stop` → `RunnerError::Cancelled` |
| Crash containment                | OS process boundary (subprocess dies, worker lives) | tokio task boundary — `JoinError::is_panic()` (B2)                 |
| Worker image deps                | `python3` + installed `cbscore` + wrapper script    | the musl `cbsbuild` binary (001 B1); no Python                     |

**Kept unchanged** (transport/lifecycle, orthogonal to the execution mechanism):
the `Supervisor` state machine
(`Accepted`/`Started`/`Revoking`/`TerminalPendingReport`), the disconnect spool
and reconnect drain, component-tarball validation/unpack/cleanup
(`component.rs`), the WS transport, and the output **batching** parameters
(`BATCH_MAX_LINES = 50`, `BATCH_FLUSH_INTERVAL = 200ms`).

**Removed**: `executor.rs` in its entirety (subprocess spawn, `setsid`
process-group isolation, SIGTERM/SIGKILL escalation, `pid`, `classify_exit_code`
incl. the `137`/`143`→`Revoked` mapping); `output.rs`'s `{"type":"result"}`
sentinel parsing and `ChildStdout` reading; the `CBS_TRACE_ID` /
`CBSCORE_CONFIG` / `CBS_BUILD_TIMEOUT` / `CBSCORE_PATH` env plumbing; and the
wrapper script plus the worker image's `python3`/`cbscore` install.

## The in-process build task

`BuildNew` dispatch (`handler.rs:443-527`) keeps its shape — validate+unpack the
tarball, send `BuildAccepted`, register with the supervisor, send `BuildStarted`
— but the executor-spawn + output-streamer pair is replaced by **two tasks** per
build: a **build task** (below) and the **completion task** that owns its handle
and produces the terminal (see Crash containment). The build task:

1. **Load config** with `Config::load` (004) from the worker's configured
   cbscore config path, then set `config.paths.components = [component_dir]`
   (the unpacked tarball dir — `cbscore-wrapper.py:152`). Config loads **first**
   because translation (step 2) reads `config.storage.registry` — the wrapper
   loads config at `cbscore-wrapper.py:142`, before translating at `:167`.
2. **Translate the descriptor.** Build a cbscore `VersionDescriptor` from the
   `BuildDescriptor` + the loaded config, mirroring the wrapper exactly (this is
   worker-side glue calling 006's `version_create_helper`):
   - `component_refs` from `components[].{name → ref}`;
     `component_uri_overrides` from `components[].{name → repo}` where `repo` is
     set;
   - `el_version` parsed from `build.os_version` (`elN` → `N`; reject otherwise
     — `cbscore-wrapper.py:58-63`);
   - `registry` from `config.storage.registry.url` (**guard**: error if storage
     or registry is unset — `cbscore-wrapper.py:148-149`);
   - `image_name`/`image_tag` from `dst_image.{name,tag}` (**guard**: non-empty
     `tag` — `cbscore-wrapper.py:127-128`);
   - `user_name`/`user_email` from `signed_off_by.{user,email}`;
   - `version`/`version_type`/`distro` from the descriptor. The descriptor is
     written to a temp file (removed on exit) that `runner::run` reads — 009
     takes a `desc_path`, matching the wrapper's temp-file handoff.
3. **Build `RunOpts`** (009): `run_name` derived from the trace id (below);
   `replace_if_exists = true` (the wrapper's `replace_run=True`); `timeout` from
   the worker's configured build timeout (default 7200 s — see below); a
   **streaming callback** log sink that feeds the batching pipeline (below); and
   the build's **`CancellationToken`**.
4. **Call `cbscore::runner::run(&desc_path, &config, opts)`** (009) and return
   its typed outcome to the completion path, which maps it to a terminal
   `BuildFinished` (below — the build task does **not** emit its own terminal).

### Run name & trace id (updates `cbsd-rs/CLAUDE.md` invariant #4)

The wrapper named the container `cbs-<trace_id-without-dashes>[:12]` with
`replace_run=True` (`cbscore-wrapper.py:234`). The port keeps exactly this
**deterministic** name — `run_name = "cbs-" + trace_id.replace('-', "")[..12]`,
`replace_if_exists = true` — which is the "deterministic name derived from the
build's trace id" 009 expects from the worker (vs the CLI's random
`gen_run_name()`). The trace id no longer travels as the `CBS_TRACE_ID`
**environment variable for a subprocess** (there is no subprocess); it is
threaded directly into the run name and the worker's tracing span. This **amends
`cbsd-rs/CLAUDE.md` correctness invariant #4** (the workspace `trace_id`
lifecycle) — the lifecycle is unchanged through dispatch and persistence, but
its final hop is an in-process argument, not a subprocess env var.

## Native result → `BuildFinished` (the payoff)

The wrapper encoded the outcome as a stdout line
`{"type":"result", exit_code, error, build_report}` that `output.rs` parsed and
`classify_exit_code` bucketed. In-process, the task consumes `runner::run`'s
typed `Result` directly and maps it to the `WorkerMessage::BuildFinished` the
supervisor already forwards:

| `runner::run` returns                              | `BuildFinishedStatus` | `build_report`                 |
| -------------------------------------------------- | --------------------- | ------------------------------ |
| `Ok(Some(report))` / `Ok(None)`                    | `Success`             | the report (or absent)         |
| `Err(RunnerError::Cancelled)` (009)                | `Revoked`             | none                           |
| `Err(RunnerError::NonZeroExit { report, stderr })` | `Failure`             | **the partial report, if any** |
| `Err(RunnerError::Podman(..))` / other             | `Failure`             | none (error text in `error`)   |

Two things this buys over the subprocess path:

- **`Revoked` is precise.** `RunnerError::Cancelled` (009) is the cancel-token
  outcome, so the worker reports `Revoked` directly — replacing the brittle
  "exit code `137`/`143` ⇒ `Revoked`" signal-number heuristic
  (`executor.rs:275-281`), which could not tell an operator revoke from an
  unrelated `SIGKILL`/OOM.
- **Partial reports survive failures.** 009's partial-report fix means a
  non-zero container exit that still produced a report carries it in
  `NonZeroExit.report`; the worker forwards it in `BuildFinished.build_report`
  **even on `Failure`**. The `BuildFinished` wire field already allows a report
  on any status (`supervisor.rs` constructs failure terminals with
  `build_report: None` today — a code choice, not a type constraint; this path
  can now populate it).

The native path must **re-home the report-size bound** that `output.rs` applied
to the parsed JSON (`MAX_REPORT_SIZE = 64 KiB`, `output.rs:35`): before placing
a report in `BuildFinished`, the completion path checks the serialized size and
drops an over-cap report (logging a warning), exactly as today. The typed path
removes the sentinel parser, not the size guard.

## Crash containment via the task boundary (resolves B2)

The subprocess gave crash containment for free: a segfault or unhandled
exception in cbscore killed only the child. In-process, a **panic** in the
cbscore orchestration would, without care, unwind through the worker. The port
contains it at the **tokio task boundary** (decision per 001 B2), but the wiring
matters — a naive "the task sends its own terminal, and `retire()` awaits it"
deadlocks the worker on a panic, because `retire()` only runs _after_ a terminal
is observed and a panicking task sends none. The corrected model uses a **single
owner** of the build-task handle and a **completion path** that is the sole
producer of the terminal:

- `runner::run` runs inside a `tokio::spawn`'d **build task** that **returns its
  typed outcome** (the `Result<Option<BuildArtifactReport>, RunnerError>`) and
  **does not** emit a `BuildFinished` itself.
- A second `tokio::spawn`'d **completion task** is the build-task handle's
  **sole owner**: it `await`s that handle exactly once and is the only place a
  terminal is produced. It maps every outcome:
  - `Ok(result)` → the mapped terminal (Success / Failure / Revoked — table
    above), via `on_output_message`;
  - `Err(e)` with `e.is_panic()` → a synthesized
    `BuildFinished { status: Failure, error: "build task panicked", .. }`, via
    the same path.

  Because the completion task emits a terminal on **every** outcome including a
  panic, the supervisor always reaches `retire()` and `active` is always cleared
  — a panicking build does not wedge the worker into a permanent "busy" state.
  (This terminal-synthesis is **new** code in the completion task; it is the
  in-process analog of the old subprocess "no result line ⇒ Failure" fallback,
  which lived in the now-deleted `output.rs` — it is not reused.)

- **Which handle the supervisor stores, and the register-then-attach order.**
  The `ActiveBuild` record holds the **completion-task** `JoinHandle` (plus the
  cancel token) — **not** the build-task handle, which the completion task alone
  owns. Crucially, the handler **registers the record _before_ spawning either
  task**, preserving today's two-step ordering (`handler.rs:491-496` registers,
  then `:519-525` attaches): `register_accepted(build_id, token, component_dir)`
  creates the record with the token but **no** handle yet; then the handler
  spawns the build task and the completion task; then
  `attach_completion_task(build_id, completion_handle)` stores the handle on the
  existing record (the analog of today's `attach_output_task`). Registering
  first is load-bearing: a **fast-failing** build (bad `os_version`, missing
  registry, empty `dst_image.tag`, `Config::load` error) can emit its terminal
  the instant the completion task runs, and the record must already exist or the
  terminal is dropped as an orphan (`supervisor.rs:270-274`) and the build hangs
  server-side. The **`CancellationToken`** is created **once** before
  registration, stored in the record at register time (the four cancel sites
  fire it), and **cloned** into the build task's `RunOpts` at spawn —
  `CancellationToken::clone` shares one cancellation state, so a cancel from any
  site reaches `runner::run`'s `select!`.
- **The handle is `Option`-and-`.take()`** for a single await, so `retire()` and
  `shutdown()` cannot both await it, and either tolerates a not-yet-attached
  `None` by skipping the await. One asymmetry vs the subprocess model: today the
  teardown handle (the executor) is stored at _register_, whereas the
  completion-task handle arrives only at _attach_, so the teardown-barrier await
  is genuinely absent in the **register→attach window**. This is **benign**:
  `shutdown()`'s only caller is a `select!` sibling of the `BuildNew` dispatch
  (`handler.rs`), so it cannot interleave between register and attach (a
  `select!` branch runs to completion before the next is polled); and even if it
  could, `token.cancel()` still fires and `runner::run` has not reached
  `podman_run` that early (it does config-load/aggregate/secrets/rewrite first,
  009), so no container is up to leak. Once attached, the handle is
  `shutdown()`'s teardown barrier (next bullet).

- The cutover **must pin `panic = "unwind"`** explicitly in the workspace
  `[profile.release]` (dev is unwind by default). Today no `[profile]` block
  exists, so the seam rests on an unpinned default; 001's failure-isolation
  invariant requires the explicit pin so a future `panic = "abort"` cannot
  silently convert a contained build panic into a worker-wide abort.
  `catch_unwind` is **not** used — it cannot soundly span `.await` points; the
  task boundary is the isolation seam.
- `retire()` no longer `.await`s the **build-task** handle (the completion task
  owns it); it `.take()`s and awaits the stored **completion-task** handle —
  which is _finishing_ when the terminal it emitted is observed (the terminal is
  enqueued on the mpsc, then the task returns), so the await is brief and
  bounded, not instant — then does resource cleanup (component dir, spool, drop
  `active`). `shutdown()` cancels the token (below) and `.take()`s/awaits the
  **same completion-task handle**, which is the teardown barrier: it returns
  only after the build task has returned, i.e. after `runner::run` performed its
  cancel-path `podman_stop`. This preserves today's "container is gone before
  the worker exits" guarantee that the subprocess `child.wait()` gave.

This is a named acceptance gate (001): a panic in host-side orchestration
surfaces as a build failure **and the worker keeps serving subsequent builds**.

## Cancellation rewrite (resolves B2)

Cancellation moves from killing a subprocess process group to firing a token:

- The supervisor holds a per-build **`CancellationToken`** (replacing the
  `BuildExecutor` handle and its `kill()`). `register_accepted` takes the token
  (the **completion-task** handle is attached afterward via
  `attach_completion_task` — see Crash containment for the register-then-attach
  order; the build-task handle is owned by the completion task, not the
  supervisor).
- **All four** current `exec.kill()` sites become `token.cancel()`:
  `on_build_revoke` (`supervisor.rs:235`), `shutdown` (`:503`), the
  spool-overflow teardown (`:627`), and the spool-write-error teardown (`:661`).
  009's `runner::run` `select!`s the token, calls `podman_stop(name = ctr_name)`
  on the known container name, and returns `RunnerError::Cancelled`.
- **Terminal arbitration (first terminal wins).** A bare `BuildRevoke` cancels
  and the completion path emits `Revoked`. But the two **spool** sites cancel
  the token only to tear the container down while having _already_ synthesized
  an authoritative `Failure` terminal (`"spool exceeded"` /
  `"spool write error"`) and set `spool_exhausted`. The completion path,
  observing the resulting `RunnerError::Cancelled`, must **not** emit a second
  (`Revoked`) terminal when the supervisor has already produced one for that
  build — it checks `spool_exhausted` / an already-set `pending_terminal` and
  suppresses. So the spool `Failure` stays authoritative; an operator
  `BuildRevoke` otherwise yields `Revoked`.
- **Benign revoke/completion race.** `BuildRevoke` ⇒ `Revoked` is not strictly
  guaranteed: if `runner::run` returns naturally (`Ok` / `NonZeroExit`) in the
  window between the token firing and `podman_stop` taking effect, the build
  reports its **real** terminal (Success/Failure), not `Revoked`. This is
  intentional and matches today's behaviour — a subprocess that exits `0` just
  as `SIGTERM` arrives reports success, not `143`⇒`Revoked`. A genuinely
  completed build's true outcome is more useful than a synthetic `Revoked`, so
  no arbitration is imposed here; the race is documented as benign rather than
  closed.
- The `setsid` process-group creation, the SIGTERM-then-SIGKILL escalation timer
  (`executor.rs:215-251`), and the `DEFAULT_SIGKILL_TIMEOUT_SECS` go away —
  podman's own `--time` grace window (009) governs the container's
  `SIGTERM`→`SIGKILL`. The supervisor's `Revoking` phase still means "cancel
  requested, awaiting the task to finish"; it awaits the completion-task handle
  instead of a child exit.

## Output streaming rewrite

The batching logic in `output.rs` (`flush_batch`, the 50-line/200ms cadence,
`BuildOutput` framing) is **retained**, but its **source changes**: instead of
reading `ChildStdout` line-by-line and sniffing a sentinel line, lines arrive
through `runner::run`'s async streaming callback (009 `RunOpts` log sink). The
callback hands each line to the batcher, which flushes `BuildOutput` messages to
the supervisor exactly as today. The `{"type":"result"}` detection and the
`WrapperResult` extraction are deleted — the terminal now comes from the typed
`Result`, not from a magic stdout line. The `stderr→stdout` `os.dup2` trick the
wrapper used is moot (the callback receives cbscore's already-merged line stream
per 003/009).

## Dependency, image, and config changes

- **`cbsd-worker` depends on the `cbscore` crate** (added to the workspace
  members alongside `cbscore-types`/`cbsbuild`, per 001). This is the link that
  makes `runner::run` an in-process call.
- **The worker image ships the musl `cbsbuild` binary** at a known path (001 B1)
  for `runner::run` to mount into the builder container, and **drops**
  `python3`, the installed `cbscore`, and `cbscore-wrapper.py`.
- The worker still needs `podman`/`buildah`/`skopeo` available (the in-process
  runner shells out to them via 003) — unchanged from today.

## Fidelity notes

- **Same two-phase build** — the in-process runner still `podman run`s the
  builder container; only the orchestrator's host process changes (Python →
  Rust, subprocess → in-process). Build isolation is preserved.
- **Descriptor translation preserved** — `version_create_helper` mapping, the
  `elN` os-version parse, the registry/`dst_image.tag` guards, and the
  `components` override all reproduce the wrapper (`cbscore-wrapper.py`), now in
  Rust against 006/004.
- **Deterministic run name** — `cbs-<trace12>`, `replace_if_exists` (matches the
  wrapper and 009's worker-caller contract); amends invariant #4 (in-process
  arg, not `CBS_TRACE_ID` env).
- **Native result** — typed `Result` replaces stdout-sentinel parsing;
  `RunnerError::Cancelled` ⇒ `Revoked` (no `137`/`143` heuristic);
  `NonZeroExit.report` ⇒ a report on `Failure` (009 partial-report fix reaches
  the server). The `MAX_REPORT_SIZE` 64 KiB report cap is **re-homed** to the
  completion path, not dropped.
- **Panic isolation** — a spawned **completion task** solely owns the build-task
  `JoinHandle` and is the sole terminal producer, so a panic
  (`JoinError::is_panic()` ⇒ synthesized `Failure`) still yields a terminal and
  `retire()` still runs (no permanent-busy wedge). The record stores the
  **completion-task** handle (`.take()`-d for one await by `retire()`/
  `shutdown()`); `shutdown()` awaits it as the container-teardown barrier. The
  cutover **pins `panic = "unwind"`** in the workspace profile (today unpinned);
  `catch_unwind` is not used. The synthesis is new code, not a reuse of the
  deleted `output.rs` fallback.
- **Cancellation** — all four `exec.kill()` sites ⇒ `token.cancel()` ⇒ 009
  `podman_stop`, replacing subprocess process-group SIGTERM/SIGKILL;
  `setsid`/escalation timer removed. The two spool-teardown sites keep their
  synthetic `Failure` authoritative (first-terminal-wins arbitration).
- **Timeout source** — the worker supplies `RunOpts.timeout` from its configured
  build timeout, **defaulting to 7200 s when unset** to preserve the wrapper's
  `CBS_BUILD_TIMEOUT` default (`cbscore-wrapper.py:233`); the worker never falls
  through to 009's 4 h default (which now governs only the CLI's no-override
  path).
- **Secrets** — the worker marshals **no** secrets (the runner owns secret
  temp-file marshalling and RAII cleanup, 009); this also sidesteps 010's dead
  `cmd_build` plaintext-secrets write entirely.
- **Supervisor state machine / spool / reconnect retained** — disconnect
  durability and the four-phase machine are unchanged. The kill **mechanism**
  changes at all four sites (`exec.kill()` ⇒ `token.cancel()`),
  `executor: Option<BuildExecutor>` + `output_task` become
  `cancel_token + completion-task JoinHandle` (the build-task handle is owned by
  the completion task, not the record), and the completion task adds terminal
  arbitration — so the supervisor is not byte-for-byte unchanged, only its
  lifecycle/spool logic.

## Testing & acceptance gates

- **Result mapping**: `Ok(Some)`→`Success`+report; `Ok(None)`→`Success`+none;
  `Cancelled`→`Revoked`; `NonZeroExit{report:Some}`→`Failure`+report;
  `NonZeroExit{report:None}` and `Podman`→`Failure`+error text.
- **Panic isolation (acceptance gate)**: a panic inside the build task yields a
  `Failure` `BuildFinished` **via the completion path**, the supervisor reaches
  `retire()` (`active` cleared), and a subsequent `BuildNew` is accepted — the
  process does not abort and is not wedged "busy". A companion build asserts
  `panic = "unwind"` is pinned (the seam is void under `panic = "abort"`).
- **Cancellation & arbitration**: `BuildRevoke` fires the token, the container
  is stopped by name (009), and the build returns `Cancelled`→`Revoked`; a
  **spool-overflow** teardown cancels the token yet the terminal stays `Failure`
  ("spool exceeded"), not `Revoked` (first-terminal-wins).
- **Shutdown teardown barrier**: `shutdown()` cancels an active build's token
  and awaits the completion-task handle, returning only after the build task has
  returned (post-`podman_stop`) — i.e. the container is stopped before the
  worker exits (the in-process analog of today's `child.wait()` on shutdown).
- **Descriptor translation**: a representative `BuildDescriptor` produces the
  same `VersionDescriptor` the wrapper would; invalid `os_version`, missing
  registry, and empty `dst_image.tag` are rejected with clear errors.
- **Fast-fail ordering**: a build that fails immediately (e.g. invalid
  `os_version`) still reaches the server — its terminal is **not** dropped as an
  orphan, proving the record is registered before the completion task can emit
  (`supervisor.rs:270-274` is not hit on this path).
- **Timeout default**: with no configured build timeout, the worker passes 7200
  s into `RunOpts` (not 009's 4 h).
- **Report cap**: an over-64 KiB report is dropped (logged) before
  `BuildFinished`, on both `Success` and `Failure` paths.
- **Streaming**: callback lines are batched (50/200ms) into `BuildOutput`
  identically to the subprocess path; no `{"type":"result"}` line ever appears
  in forwarded output.
- **Supervisor lifecycle retained**: existing supervisor/spool/reconnect tests
  pass with the token+JoinHandle substitution (no behavioural change to
  disconnect durability).
- **End-to-end (acceptance gate, 001)**: a full build runs through the worker's
  in-process path with the Python wrapper retired; report and logs reach the
  server.
