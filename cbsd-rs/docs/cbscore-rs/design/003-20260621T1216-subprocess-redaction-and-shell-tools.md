# 003 — Subprocess execution, secret redaction & shell-tool wrappers

This is the reference design for the lowest layer of the `cbscore` library: the
async subprocess primitive, the secret-redaction machinery that keeps
credentials out of logs and errors, and the four thin shell-tool wrappers built
on top (`git`, `podman`, `buildah`, `skopeo`). Every higher subsystem reaches
external tools only through this layer. Read 001 for the crate layout and 002
for the error-taxonomy split.

Source of truth: `cbscore/utils/__init__.py` (the primitive + redaction),
`cbscore/utils/git.py`, `cbscore/utils/podman.py`, `cbscore/utils/buildah.py`,
`cbscore/images/skopeo.py`.

## Scope and ownership

This document specifies, as the single source of truth:

- the async subprocess primitive (`async_run_cmd` equivalent);
- the redaction types (`CmdArg`, `SecureArg`, `Password`, `PasswordArg`,
  `SecureUrl`) and the log/exec split;
- the **conventions** every shell-tool wrapper follows, and each wrapper's
  operation **interface** (signatures + error type + redaction integration).

The **deep per-operation semantics** of each tool belong to the subsystem that
drives it and are specified there, not duplicated here: `podman_run`'s
mount/network/timeout wiring and the two-phase cancellation hook → 009 (runner);
`BuildahContainer`'s commit/push/sign sequence → 008 (containers & images);
skopeo's image-exists/copy/sign flows → 008; git clone/checkout/worktree/apply
as used by component preparation → 007.

`utils::subprocess` and `utils::git` are **lift-out candidates** for a future
shared-primitives crate (001): they import only primitives and generics, carry
their own tracing targets, and depend only on `tokio`, `tracing`, `thiserror`,
`regex`, `camino`, `which`.

## The subprocess primitive

Python exposes two primitives: an async `async_run_cmd` (used everywhere except
skopeo) and a sync `run_cmd` (used only by skopeo). The Rust port **unifies on
one async primitive** — there is a single runtime (tokio), so skopeo becomes
async like the rest (no behavioral change; see Fidelity notes). The Python
`_reset_python_env` PATH-scrubbing workaround is **not ported** (001/002):
`cbsbuild` runs from no venv, so it is moot.

```rust
struct RunOpts<'a> {
    cwd: Option<&'a Utf8Path>,
    timeout: Option<Duration>,
    extra_env: &'a [(String, String)],   // merged over the inherited env
    out_cb: Option<&'a OutCb>,            // async per-line callback
}

struct CmdOutput { code: i32, stdout: String, stderr: String }

async fn run_cmd(args: &[CmdArg], opts: RunOpts)
    -> Result<CmdOutput, CommandError>;
```

Behavior, matching `async_run_cmd`:

- **Environment**: inherits the process environment, then applies `extra_env` on
  top. (No PATH scrubbing.)
- **Spawn**: `tokio::process::Command` with `stdout`/`stderr` piped; the program
  and arguments are the **plaintext** rendering of the `CmdArg`s (see
  redaction).
- **Concurrent line streaming**: stdout and stderr are read concurrently, line
  by line. If `out_cb` is set, each line is awaited through it and
  `stdout`/`stderr` in the result are left empty; otherwise lines are collected
  into the returned strings. (Mirrors Python's `read_stream`.)
- **Timeout & cancellation**: the spawn+wait is wrapped in
  `tokio::time::timeout`. On elapse — and on task cancellation (the future being
  dropped, or a cancellation signal per 001/009) — the child is killed and
  reaped before the error propagates. Timeout surfaces as a distinct
  `CommandError` variant so callers can tell it apart from a non-zero exit
  (Python conflates these; the Rust port separates them, as the Python FIXME at
  `utils/__init__.py:258` itself recommends).
- **Result**: a non-zero exit is **not** an error of this primitive — it returns
  `CmdOutput { code, .. }`; each wrapper decides whether a non-zero code is
  fatal (matching the Python wrappers, which inspect `rc`). Spawn failure
  (binary missing, etc.) is a `CommandError` — a **deliberate divergence**:
  Python's async `async_run_cmd` does not wrap `create_subprocess_exec`
  (`utils/__init__.py:225`), so a spawn `OSError` propagates raw; the Rust port
  turns it into a typed `CommandError`.

The plaintext rendering is used **only** at spawn; every log line, debug print,
and error message uses the redacted rendering.

## Secret redaction

The redaction contract is **type-driven first, pattern-driven as a backstop**.
An argument that carries a secret is wrapped in a type that does not expose its
plaintext through any formatting trait, so it is structurally impossible to log
the plaintext by formatting the argument; a string-pattern pass additionally
censors credential-bearing flags that arrive as plain strings.

```rust
trait SecureArg: Send + Sync {       // deliberately NOT `fmt::Debug`/`Display`
    fn plaintext(&self) -> String;   // used ONLY when spawning
    fn redacted(&self) -> String;    // used for logs and errors
}

enum CmdArg {
    Plain(String),
    Secure(Arc<dyn SecureArg>),
}
```

- `SecureArg` deliberately does **not** require `Debug` or `Display`, so a
  secret type cannot be `{:?}`/`{}`-formatted directly — there is no
  `#[derive(Debug)]` that could leak it, and the compiler enforces this rather
  than review discipline. `CmdArg`'s hand-written `Debug`/`Display` emit
  `redacted()` for `Secure` and the (pattern-redacted) string for `Plain`, so
  `format!("{:?}", arg)` can never leak a secret. Concrete secret types expose
  their censored rendering through `redacted()` (and may impl `Display` as
  `redacted()` for logging). The primitive renders plaintext for spawn via an
  explicit `plaintext()` path, never via any formatting trait.

Concrete implementations mirror the Python types:

| Rust                            | Python        | `redacted()`                       | `plaintext()`        |
| ------------------------------- | ------------- | ---------------------------------- | -------------------- |
| `Password(String)`              | `Password`    | `<CENSORED>`                       | the value            |
| `PasswordArg { arg, Password }` | `PasswordArg` | `arg=<CENSORED>`                   | `arg=<value>`        |
| `SecureUrl { template, args }`  | `SecureURL`   | template with secure args censored | template with values |

`SecureUrl` is the credentialed-git-URL case: a template like
`https://{user}:{pass}@host/repo` whose `{pass}` argument is a `Password`, so
the clone URL is censored in logs but plaintext at spawn. Git `repo` arguments
are therefore `CmdArg`s, not bare strings.

**Pattern backstop.** For credential flags passed as plain strings, a sanitiser
censors the value in the logged/redacted rendering. Python covers the two-token
forms `--pass <v>` and `--passphrase <v>`, plus inline `--passphrase=<v>` — but
**not** inline `--pass=<v>`, because its regex requires the `phrase` suffix
(`utils/__init__.py:121,130`). The Rust port covers all of those and
additionally `--password` and `-p` (two-token and inline) — strictly broader
than Python. This only affects what is logged; it never changes what is spawned,
so broadening it is safe.

## Shell-tool wrapper conventions

Each of the four tools is a thin module over the primitive following one
pattern: a base runner that prepends the program name and threads `out_cb`; a
tool-specific `thiserror` error (defined with the tool, per 002's
IO-layer-errors-live-with-their-subsystem rule); a dedicated tracing target
(`cbscore::utils::git`, `…::podman`, `…::buildah`, `cbscore::images::skopeo`);
credential arguments wrapped in `Password`; and `Result`-returning operations
that map a non-zero exit to the tool's error. The catalogue below is the
interface; deep semantics are owned by the consuming subsystem noted in Scope.

### git (`utils::git`)

`GitError { retcode: i32, msg }` (plus `GitConfigNotSet`). Base
`run_git(args, path: Option<&Utf8Path>)` prepends `git` and, when `path` is
given, `-C <path>`; returns stdout or `GitError`. Operations (each async):

- `get_git_user() -> (String, String)` — `git config user.name` / `user.email`;
  `GitConfigNotSet` when empty. _(first consumer: C1)_
- `get_git_repo_root() -> Utf8PathBuf` — `git rev-parse --show-toplevel`. _(C1)_
- `git_clone(repo: CmdArg, base, name) -> Utf8PathBuf` — clone `--mirror` to
  `<base>/<name>.git`, or update in place if a valid mirror exists
  (`remote set-url` + `remote update`); `repo` is `CmdArg` (may be a
  `SecureUrl`). _(C3, prepare)_
- `git_checkout(repo_path, ref, worktrees_base) -> Utf8PathBuf` — add a worktree
  on a new branch named `ref` with `/`→`--` and a random hex suffix; returns the
  worktree path. _(C3)_
- `git_remove_worktree(repo_path, worktree_path)` — `worktree remove --force`.
  _(C3)_
- `git_apply(repo_path, patch_path)` — `git apply <patch>`. _(C3)_
- `git_get_sha1(repo_path) -> String` — `git rev-parse HEAD`. _(C3)_

Other Python git helpers (`get_git_modified_paths`, `git_fetch`, `git_pull`,
`git_cherry_pick`, `git_get_current_branch`) are not used by the core
build/versions paths; they are ported only if a consuming subsystem needs them,
and specified there.

### podman (`utils::podman`)

`PodmanError { retcode: i32, msg }`. Two operations:

- `podman_run(image, RunArgs) -> CmdOutput` — builds `podman run` with a fixed
  prelude (`--security-opt label=disable`, `--cidfile <tmp>`, `--attach stdout`,
  `--attach stderr`) plus optional `--name`, `--userns keep-id`, `--timeout`,
  `--security-opt seccomp=unconfined`, `--replace`, repeated
  `--env`/`--volume`/`--device`, `--network host`, and `--entrypoint`; streams
  via `out_cb`. **On timeout/cancel it reads the cidfile and calls
  `podman_stop(cid)`** before erroring — this is the hook the runner's
  cancellation builds on (009). **Two timeout deadlines race on elapse** (as in
  Python, which passes `timeout` to both): podman's own `--timeout` may kill the
  container first, yielding a non-zero exit returned as `CmdOutput`; or the
  outer await-deadline fires first, doing the cidfile→`podman_stop` and
  surfacing the distinct timeout error. Callers must therefore treat _either_ a
  timeout error _or_ a non-zero exit as a possible timeout outcome — 009 maps
  the former to `RunnerError::Podman` and the latter to
  `RunnerError::NonZeroExit`.
- `podman_stop(name: Option<&str>, timeout)` —
  `podman stop --time <t> <name | --all>`.

The full `RunArgs` shape and the runner's use of it (mounts, HOME, `CBS_DEBUG`,
the `cbsbuild` entrypoint) are specified in 009.

### buildah (`utils::buildah`)

`BuildahError`. Base `buildah_run(cmd, cid, args, with_divider, out_cb)`
prepends `buildah`, appends the container id and (optionally `--`-divided)
arguments. The container is modelled as a
`BuildahContainer { cid, version_desc, is_committed }` with `set_config`,
`copy`, `run` (`run --isolation chroot -- …`), and `finish`. `finish` commits
(`commit --squash`), obtains registry creds from the secrets manager, and pushes
(`push --digestfile … [--creds <Password>] …`) — the `--creds` value is a
`Password`. `buildah_new_container(desc)` does `buildah from <distro>` and sets
the initial OCI config. The commit/push/transit-sign sequence is specified
in 008.

### skopeo (`images::skopeo`)

`SkopeoError`, plus `ImageNotFound` and `UnknownRepository`. Base `skopeo(args)`
prepends `skopeo`. Operations:

- `skopeo_image_exists(img, secrets, tls_verify) -> bool` — wraps
  `skopeo_inspect`; `false` on `ImageNotFound`. _(first consumer: C6)_
- `skopeo_inspect(img, secrets, tls_verify) -> String` —
  `skopeo inspect --tls-verify=<b> --creds <Password> docker://<img>`;
  `ImageNotFound` on exit 2 / "not found". _(C6)_
- `skopeo_get_tags(img) -> SkopeoTagList` — `skopeo list-tags`; parses
  `{ "Repository", "Tags" }` (a small parse type owned here);
  `UnknownRepository` when the repo is absent. _(008)_
- `skopeo_copy(src, dst, dst_registry, secrets, transit)` —
  `skopeo copy --dest-creds <Password> docker://src docker://dst`, then
  transit-sign if configured. _(008)_

`--creds` / `--dest-creds` values are `Password`s. Python's skopeo helpers are
synchronous (`run_cmd`); the Rust port makes them async on the one runtime.

## Fidelity notes

- **skopeo sync → async.** The only behavioral-shape change in this layer:
  Python skopeo uses the sync primitive; Rust uses the async one. No observable
  difference beyond running on the tokio runtime.
- **`git -C <path>`.** The wrapper invokes git with `-C <path>` exactly as
  Python does; this is the ported tool invocation, internal to cbscore.
- **Timeout vs. exit code separated.** The Rust primitive distinguishes a
  timeout/cancel error from a non-zero exit, resolving the Python FIXME.
- **`_reset_python_env` dropped** — no venv on `PATH` for `cbsbuild`.
- **Worktree branch naming** uses a random suffix (Python
  `secrets.token_hex(5)`); the Rust port uses a CSPRNG-backed equivalent (the
  value is not security-sensitive, only collision-avoiding).

## Testing

- **Redaction is structural**: a test that `format!("{:?}", cmd)` and the
  primitive's logged form contain `<CENSORED>` and never the plaintext, for
  `Password`, `PasswordArg`, and `SecureUrl`; and that the pattern backstop
  censors `--pass`/`--passphrase`/`--password`/`-p` values.
- **Plaintext reaches exec**: a test (e.g. via `printenv`/`echo` or a stub) that
  the spawned process receives the plaintext while the captured log shows the
  redacted form.
- **Primitive behavior**: concurrent stdout/stderr line streaming through
  `out_cb`; non-zero exit returned (not errored); spawn failure errors; timeout
  kills the child and returns the timeout variant; `extra_env` is merged over
  the inherited environment.
- **Wrapper error mapping**: each tool maps a non-zero exit to its error type
  carrying the captured stderr (and `retcode` for git/podman).
