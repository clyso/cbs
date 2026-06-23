# cbscore-rs Roadmap

Forward-looking items deferred from the cbscore Rust port. Each entry records
the motivation, the affected area, and the trigger for picking it up. The items
here are behaviors or quirks the port **reproduces faithfully** (rather than
changes mid-port) and that warrant a deliberate decision later.

See also the workspace-wide `cbsd-rs/docs/ROADMAP.md`.

## Priority labels

`C` (Critical), `H` (High), `M` (Medium), `L` (Low) — a coarse ordering hint,
not a commitment.

## Builder pipeline (design 007)

### Components silently dropped from the release when no release-RPM script

- Priority: M
- Origin: design 007 review (finding F2), verified against `builder.py:576-582`
  and `releases/utils.py`.
- Motivation: when a component has no `get_release_rpm.sh` (or the script yields
  nothing), `get_component_release_rpm` returns `None` and the orchestrator
  **drops that component from the release descriptor entirely** — even though
  its RPMs were already built, signed, and uploaded to S3. The RPMs become
  orphaned (present in S3, unreferenced by any release), and the release
  silently omits a component that was actually built. The port reproduces this
  Python behavior (with a clear error log) rather than changing it mid-port.
- Scope: decide the correct behavior — e.g. record the component in the release
  with an absent/optional `release_rpm_loc` (making
  `ReleaseRPMArtifacts.release_rpm_loc` optional, a wire change), or fail the
  build loudly instead of silently dropping. Either is a semantic/wire-format
  decision, out of scope for a faithful port.
- Trigger: before a release pipeline runs components that legitimately lack a
  release-RPM script, or during a future revision of the `ReleaseDesc` wire
  format.

### `get_release_rpm.sh` runs with no working directory

- Priority: L
- Origin: design 007 review (finding F3) — `releases/utils.py` versus the other
  component scripts in `builder/{rpmbuild,utils}.py`.
- Motivation: `install_deps.sh`, `build_rpms.sh`, and `get_version.sh` all run
  with their working directory set to the component worktree, but
  `get_release_rpm.sh` runs with **no cwd** (the builder process's default
  directory). A release-RPM script that assumed it ran inside the worktree would
  misbehave; the inconsistency is latent today only because the current scripts
  do not depend on cwd. The port reproduces the Python behavior (no cwd for this
  one script).
- Scope: investigate whether `get_release_rpm.sh` should run in the component
  worktree like the other three scripts, and either align the contract or
  document the difference as deliberate in the component-script contract.
- Trigger: when adding or revising a component's `get_release_rpm.sh`, or when
  publishing the component-script contract for component authors.
