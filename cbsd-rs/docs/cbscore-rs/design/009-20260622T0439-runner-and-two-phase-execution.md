# 009 — Runner & two-phase execution

This is the reference design for the host-side runner of the `cbscore` library:
the orchestrator that prepares inputs, spawns the builder container, streams its
output, and recovers the build report. It owns the **two-phase boundary** — what
the host does versus what runs inside the container. Read 001 for the
binary-mount and failure-isolation invariants (B1/B2) and the build-report
round-trip (invariant 5), 003 for the `podman_run`/`podman_stop` wrapper, 004
for `Config`/`SecretsMgr`, and 007 for `Builder.run` (the in-container half this
spawns).

Source of truth: `cbscore/runner.py` and the entrypoint it replaces
(`cbscore/_tools/cbscore-entrypoint.sh`).

The in-container build (`cbsbuild runner build` → `Builder.run`) is 007. This
document's two consumers — the `cbsbuild build` CLI command and the in-process
`cbsd-worker` — are detailed in 010 and 011; 009 specifies the `runner::run`
they both call.

## The two phases

1. **Host** (`runner::run`, this document): read the descriptor, aggregate
   components, marshal secrets and a path-rewritten config to temp files, then
   `podman run` the builder image with everything mounted.
2. **Container**: the mounted `cbsbuild` runs as PID 1 and re-enters as
   `cbsbuild runner build …` → `Builder.run` (007), writing `build-report.json`
   to the scratch mount.
3. **Host** again: read the report back across the scratch mount, clean up, and
   surface the result.

## `runner::run` — host orchestration

Mirrors `runner.py:runner`. Conceptually:

```rust
async fn run(
    desc_path: &Utf8Path,
    config: &Config,
    opts: RunOpts,            // run_name?, replace_if_exists, timeout,
                              // skip_build, force, tls_verify, cancel token,
                              // log sink (file path XOR callback)
) -> Result<Option<BuildArtifactReport>, RunnerError>;
```

Steps:

1. **Validate inputs** — the descriptor file exists and parses
   (`VersionDescriptor::read`, 002; `NoSuchVersionDescriptor` /
   `InvalidVersionDescriptor` otherwise), and the `cbsbuild` artifact to mount
   exists and is executable (replacing Python's entrypoint-script validation).
2. **Aggregate components** — copy every subdirectory of each
   `config.paths.components` entry into one temp directory (Python's
   `_setup_components_dir`), so the container sees a single
   `/runner/components`. Removed on exit.
3. **Marshal secrets** — `config.get_secrets()` (004) written to a temp secrets
   file; removed on exit.
4. **Rewrite config for the container** — clone `config` with in-container
   paths: `secrets = [/runner/cbs-build.secrets.yaml]`,
   `vault = /runner/cbs-build.vault.yaml` (if set), `scratch = /runner/scratch`,
   `scratch_containers = /var/lib/containers`,
   `components = [/runner/components]`, and `ccache = /runner/ccache` (if set).
   Write it to a temp config file (removed on exit). (Python also rewrites an
   in-container `logging` path to an unmounted location; the port instead clears
   `logging` to `None` — see Fidelity notes.)
5. **Spawn** — `podman_run` (003) launches **the image named by `desc.distro`**
   (the descriptor's distro tag, e.g. `rockylinux:9`) with the mount table, env,
   devices, and flags below, streaming output through the log sink. The mounted
   `cbsbuild` overrides the image's entrypoint; the image supplies only the
   build toolchain base.
6. **Recover the report** — read `build-report.json` from the host scratch
   directory **before** the return-code check, then unlink it (below).
7. **Finish** — clean up **all** temp inputs (the aggregated components dir, the
   secrets file, and the config file) on **every** path via RAII guards (see
   Fidelity notes — a security fix over Python). Then: a **non-zero container
   exit** returns `RunnerError::NonZeroExit { report, stderr }`, where `report`
   is the partial report read in step 6 (`Some` if `Builder` wrote one, else
   `None`); a **timeout or podman failure** (`podman_run` raised) returns
   `RunnerError::Podman`; a **cancellation** (the `select!` dropped `podman_run`
   after the token fired and the runner stopped the container by name) returns
   `RunnerError::Cancelled`. None of the three error paths carries a report
   (step 6 never ran — see Errors and the round-trip section); success returns
   the report (`None` if absent).

## Binary mount — no entrypoint, no source tree (resolves B1)

Python mounts the cbscore **source tree** at `/runner/cbscore` and a shell
**entrypoint** that installs `uv`, builds a venv, `uv tool install`s the wheel,
and finally runs `cbsbuild`. The Rust port **mounts the compiled musl `cbsbuild`
binary** at `/runner/cbsbuild` and runs it as PID 1 directly:

- `--entrypoint /runner/cbsbuild`; the artifact is the explicit musl build
  shipped in the worker image (001 B1), never "self".
- **Gone**: the `/runner/cbscore` source mount, the `/runner/entrypoint.sh`
  mount, and the entire uv/venv/wheel bootstrap. `--cbscore-path` and
  `-e/--cbs-entrypoint` are dropped (001).
- The in-container invocation (the `podman run` trailing args, i.e. `cbsbuild`'s
  argv) is:

  ```
  --config /runner/cbs-build.config.yaml runner build \
    --desc /runner/<descriptor>.json --tls-verify=<bool> [--skip-build] [--force]
  ```

  This is the `runner build --desc <mount>` shape (resolving 001's H2; the
  internal CLI contract is owned by 010). Both ends are the same binary, so the
  host emits exactly what the in-container parser expects.

## Mount table, env, flags

| host source                | container path                   | notes                                                    |
| -------------------------- | -------------------------------- | -------------------------------------------------------- |
| `cbsbuild` (musl)          | `/runner/cbsbuild`               | **new** — PID 1; replaces the source + entrypoint mounts |
| descriptor file            | `/runner/<name>.json`            | passed via `--desc`                                      |
| rewritten config           | `/runner/cbs-build.config.yaml`  | `--config`                                               |
| temp secrets               | `/runner/cbs-build.secrets.yaml` |                                                          |
| temp vault config          | `/runner/cbs-build.vault.yaml`   | only if `config.vault` set                               |
| `paths.scratch`            | `/runner/scratch`                | report crosses here                                      |
| `paths.scratch_containers` | `/var/lib/containers:Z`          | buildah/skopeo storage                                   |
| aggregated components      | `/runner/components`             |                                                          |
| `paths.ccache`             | `/runner/ccache`                 | only if set                                              |

- **env**: `CBS_DEBUG=<1|0>` derived from the host's effective debug state
  (resolving H3 — the in-container `cbsbuild` reads it via clap's `env`,
  replacing the entrypoint's `CBS_DEBUG`→`--debug` translation). **`HOME` is not
  host-set**: the entrypoint set `HOME=/runner` only when it was unset or `/`
  (so a normal image keeps `HOME=/root`), which `-e` cannot express
  conditionally; so the conditional moves _into the binary_, applied **only on
  the in-container `runner build` entry** (set `HOME=/runner` iff unset or `/`).
  This is scoped to `runner build` precisely because that is the path that runs
  as PID 1 in the builder image; the host-side `cbsbuild build` invocation of
  the same binary runs in the operator's normal shell and must **not** touch
  `HOME`. 010 (CLI surface) owns wiring this startup hook onto the
  `runner build` subcommand. An unconditional `-e HOME=/runner` would wrongly
  override an image's `/root`.
- **devices**: `/dev/fuse` (for buildah in-container).
- **flags** (from `podman_run`, 003): `--security-opt label=disable`,
  `--security-opt seccomp=unconfined`, `--network host`, no user namespace,
  `--cidfile <tmp>`, `--name <run-name>`, `--timeout`, and `--replace` when
  `replace_if_exists`.

## Timeout & cancellation (resolves B2)

These are **two distinct mechanisms** with two distinct owners; conflating them
is the trap.

- **Timeout is `podman_run`'s job** (003), not 009's. Python passes the build
  `timeout` to _both_ podman's own `--timeout` flag (`podman.py:78`) _and_ the
  `async_run_cmd` wait it wraps the process in (`podman.py:115`); on elapse it
  reads the cidfile and `podman stop`s the container, then raises `PodmanError`.
  The port keeps this entirely inside `podman_run`: 009 forwards `opts.timeout`
  and lets the wrapper own the dual `--timeout`/await-deadline behaviour. 009
  does **not** add a second `tokio::time::timeout` around the call — that would
  be a third, redundant deadline racing the two `podman_run` already owns.
- **Cancellation is 009's job** (B2 delegates the detail here). `runner::run`
  `select!`s the `podman_run` future against **only** the caller-supplied
  **`CancellationToken`** (the worker cancels a build through it; 011). When the
  token fires, the runner **explicitly calls `podman_stop(name = ctr_name)`** —
  it stops the container by its known `--name`, because dropping the
  `podman_run` future runs no async cleanup (Rust has no async `Drop`).
  `podman stop` sends `SIGTERM` to the `cbsbuild` PID 1, which shuts down within
  the `podman stop --time` grace window before `SIGKILL`. Because the `select!`
  **drops** the `podman_run` future rather than letting it return, no
  `PodmanError` is produced on this branch; after stopping the container `run`
  returns its own **`RunnerError::Cancelled`** (not `Podman` — see Errors). This
  replaces Python's `wait_for`-cancellation (where `CancelledError` propagated
  into `podman_run`'s own handler) and the worker's old
  kill-the-subprocess-group.

`podman_stop(name = ctr_name)` always stops **by name** here; the runner never
uses the wrapper's `name = None` form (which maps to `podman stop --all` and
would tear down unrelated containers). Default timeout matches Python (4 h),
overridable per build.

## Build-report round-trip (invariant 5 / resolves H1)

The report crosses the boundary as a **file on the scratch mount**, not a return
value. The in-container `Builder` writes `build-report.json` to
`/runner/scratch` (007, on both the skipped and full-build paths). When
`podman_run` **returns** (any exit code), the host reads
`<paths.scratch>/build-report.json` and parses it **before** the return-code
check, then unlinks it.

**Partial-report fix (broken Python).** Python reads the report before the rc
check — the comment says this is "so partial reports are captured when RPMs
uploaded but the container push failed" — but on `rc != 0` it then raises
`RunnerError` carrying only stderr, **discarding the report it just read**
(`runner.py:320-338`), so the read-before-rc is dead effort. The port fixes this
with a **single** mechanism: the non-zero-exit `RunnerError` variant **carries
an `Option<BuildArtifactReport>`** (see Errors). `run`'s signature stays
`Result<Option<BuildArtifactReport>, RunnerError>` — success returns
`Ok(report)`, a non-zero exit returns
`Err(RunnerError::NonZeroExit { report, stderr, .. })` — so the report rides
exactly one of the two arms and never both. The unlink-after-read is preserved.

**Where the carry does and does not apply** (the read is only on the
container-returned path):

- **Container ran and exited non-zero** — `podman_run` returned an rc; the
  report was read before the rc check. If the in-container `Builder` wrote a
  (possibly partial) report before failing, `RunnerError::NonZeroExit.report` is
  `Some`; otherwise `None`. This is the path the fix targets.
- **Timeout / podman failure** — `podman_run` raised (`PodmanError`) instead of
  returning, so the read-before-rc never executes; `RunnerError::Podman` holds
  no report.
- **Cancellation** — the `select!` dropped `podman_run` (no return, no
  `PodmanError`); `run` stops the container and returns
  `RunnerError::Cancelled`, which holds no report.

Both raise-/drop-paths are correct: a build killed mid-flight by the deadline or
the cancel token has not reached `Builder`'s end-of-run report write, so no
report exists to recover.

## Run name & two callers

`gen_run_name(prefix = "ces_")` builds a container name as the prefix plus ten
random lowercase ASCII characters (Python samples with replacement via
`random.choices`; the port uses a CSPRNG-backed equivalent — the value is
collision-avoiding, not security-sensitive). `runner::run` takes an optional
`run_name` (defaulting to `gen_run_name()`); the **worker** passes a
deterministic name derived from the build's trace id (011), the **CLI** uses the
generated one (010). Both consume the same `runner::run`: the CLI streams to a
log file or stdout, the worker supplies a streaming callback and a cancellation
token and consumes the returned `Result` + report natively (the whole point of
the in-process integration, 001/011).

## Errors

`RunnerError` is the IO-layer error for this subsystem (lives in `cbscore`, per
002). Its variants:

- **`NonZeroExit { report: Option<BuildArtifactReport>, stderr: String }`** —
  the container ran to completion but exited non-zero. Carries the captured
  stderr **and the partial report** read before the rc check (`Some` when
  `Builder` wrote one before failing, else `None`; see the round-trip section).
  This is the only report-bearing variant.
- **`Podman(PodmanError)`** — `podman_run` failed or raised on timeout/the
  cidfile→stop path; no report (the read never ran).
- **`Cancelled`** — the caller's `CancellationToken` fired; the `select!`
  dropped `podman_run` (so there is no `PodmanError` to wrap) and the runner
  stopped the container by name. A distinct variant — not `Podman` — so the
  worker can tell an operator-requested cancel apart from a genuine podman
  failure; no report.
- **config/secrets marshalling errors** — surfaced while preparing the temp
  config and secrets files; no report.

A missing/invalid descriptor surfaces as `NoSuchVersionDescriptor` /
`InvalidVersionDescriptor` (002/006) during input validation, before any
container is spawned.

## Fidelity notes

- **No entrypoint, no source mount** (B1) — the musl `cbsbuild` is mounted and
  run as PID 1; the uv/venv/wheel bootstrap and `/runner/cbscore` are gone;
  `--cbscore-path` / `-e/--cbs-entrypoint` dropped (001). Python's
  entrypoint-script symlink/realpath check is **moot here** — the mounted
  artifact is an operator-shipped binary at a fixed path in the worker image,
  not a script a build can swap; step 1's exists-and-executable check is the
  whole validation.
- **`CBS_DEBUG` via env** (H3) — forwarded as `-e CBS_DEBUG=<1|0>`; the
  in-container binary reads it (no `--debug`-flag translation).
- **`HOME` set conditionally on the `runner build` entry** (iff unset or `/`),
  replicating the entrypoint — applied only on the in-container path that runs
  as PID 1, never on the host-side `cbsbuild build` invocation of the same
  binary, and never host-set via `-e` (which would override an image's `/root`).
  010 wires the startup hook.
- **In-container CLI shape** `runner build --desc <mount> --tls-verify=…` (H2) —
  host emit and in-container parse agree (010).
- **Build-report round-trip** — read-before-rc + unlink (invariant 5 / H1); the
  partial report now **rides the failure path** rather than being discarded on
  `rc != 0` (Python bug fixed).
- **Cancellation vs timeout — two owners** — the build **timeout** lives inside
  `podman_run` (003: dual `--timeout` + await-deadline, then cidfile→stop), as
  in Python (`podman.py:78` and `:115`); 009 adds **only** a `select!` of the
  run against the external `CancellationToken`, then an explicit
  `podman_stop(name = ctr_name)` (B2). No second `tokio::time::timeout`, no
  async-`Drop`, no `--all` stop. Default 4 h.
- **Temp-file cleanup fixed (broken Python, security)** — Python's `finally`
  cleans only the components dir, leaking the temp **secrets file (plaintext
  creds)** and config on the success and `PodmanError` paths
  (`runner.py:286-316`); the port guards every temp file with RAII so all are
  removed on all paths.
- **In-container logging path dropped** — Python rewrites `config.logging` to
  `/runner/logs/cbs-build.log` but never mounts `/runner/logs`, so that log is
  ephemeral; the real capture is the host-side streaming callback. The port
  omits the rewrite and instead **clears `config.logging` to `None`** in the
  path-rewritten container config (step 4), rather than leaving it pointed at an
  unmounted path — the in-container `cbsbuild` then logs to stderr/stdout, which
  the host streams.
- **`reset_python_env` is moot** here — `cbsbuild` runs no venv (003/007).
- **In-container path rewrite** (`/runner/...`) and the `:Z` SELinux label on
  the containers-storage mount are preserved (on-disk-layout parity, invariant
  3).

## Testing

- **Mount/argv assembly**: `run` builds the expected mount table, env
  (`CBS_DEBUG`; **no host-set `HOME`**), devices, flags, and the in-container
  argv
  (`--config … runner build --desc … --tls-verify=… [--skip-build] [--force]`)
  for a given config/descriptor; the `cbsbuild` mount uses the explicit artifact
  path, never "self".
- **Config rewrite**: the container config has the `/runner/...` paths;
  vault/ccache mounts appear only when configured.
- **Report round-trip**: a report written to the host scratch is read and
  returned; a present report on a **non-zero** exit **rides the `RunnerError`**
  (not discarded); the scratch file is unlinked afterward.
- **Cancellation**: firing the token makes the `select!` drop `podman_run`,
  trigger an explicit `podman_stop(name = ctr_name)`, and return
  `RunnerError::Cancelled`; no container is leaked.
- **Timeout**: elapsing the deadline is handled **inside** `podman_run` (003:
  `--timeout` + the await-deadline, then its own cidfile→stop) and surfaces as
  `RunnerError::Podman`; 009 does **not** run its explicit stop here; no
  container is leaked.
- **Cleanup**: all temp inputs (components dir, **secrets file**, config) are
  removed on success **and** on every error path (incl. `PodmanError`) — no
  plaintext-secret file is left on disk.
- **Validation**: a missing descriptor → `NoSuchVersionDescriptor`; a missing
  `cbsbuild` artifact → a clear `RunnerError`.
