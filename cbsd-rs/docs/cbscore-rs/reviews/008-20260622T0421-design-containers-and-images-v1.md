# Review 008 v1 — Design: Containers & images

Adversarial review of
`cbsd-rs/docs/cbscore-rs/design/008-20260622T0413-containers-and-images.md`.

**Scope.** The `ContainerBuilder` orchestrator, the `ComponentContainer`
handler, the `ContainerDescriptor` (`container.yaml`) type and repos union, the
buildah commit/push/sign sequence, cosign transit signing, and the standalone
image sync. Verified line-by-line against the Python source of truth:
`containers/{build,component,repos,desc,__init__}.py`,
`images/{signing,sync,skopeo,__init__}.py`, `utils/{containers,buildah}.py`,
`utils/__init__.py`, `utils/secrets/{mgr,registry}.py`, `releases/desc.py`,
`versions/desc.py`.

**Bottom line.** The design is a faithful, well-organised summary of a genuinely
intricate subsystem. The repos-union model, the descriptor selection, the build
sequence, the `dnf` flag distinctions, and the `VAULT_TOKEN`-via-environment
redaction claim are all accurate. However, the source it summarises contains
**two latent bugs** the design narrates as clean behaviour, and the port will
silently reproduce them unless the design names them. The headline issue is that
`finish` does not check the buildah **push** return code — a failed push reports
success (and, with signing on, signs the wrong reference). A second is that
`can_sign`, typed `-> bool`, **raises** an uncaught `ValueError` when no
registry creds match. Both are in-scope (the finish/sign sequence 008 explicitly
owns) and both warrant an explicit "the port must decide: reproduce or fix" note
in the design. Several smaller fidelity gaps round out the list.

## Verdict

**go-with-changes.** No structural objection to the design; the model and the
sequence are sound. Before the port is built, the design must (1) name the
unchecked-push-rc bug and state the intended port behaviour, (2) correct the
`can_sign`/`sign` "requires them" wording to reflect the uncaught `ValueError`
contract violation, (3) mark `cond` as parsed but never evaluated, and (4) fix
the `get_components` var-source enumeration. The remaining items are precision
nits that can be folded into the same pass.

## Confidence score

| Item                                                              | Points | Description                                                                                            |
| ----------------------------------------------------------------- | -----: | ------------------------------------------------------------------------------------------------------ |
| Starting score                                                    |    100 |                                                                                                        |
| F1 — unchecked push `rc` in `finish` not surfaced (D8 spec dev.)  |    -10 | Design narrates commit→push→sign as clean; push rc is never checked, so a failed push returns success  |
| F2 — `can_sign`/`sign` "requires creds" mischaracterised (D8)     |     -5 | `can_sign` is typed `-> bool` but raises uncaught `ValueError`; not a graceful gate                    |
| F3 — `cond` field modelled but not flagged as dead (D11 doc gap)  |     -5 | `ContainerPackagesEntry.cond` is parsed and never evaluated anywhere; design shows `cond?` unqualified |
| F4 — `get_components` var-source enumeration incomplete (D10)     |     -5 | Prose omits that `git_ref` and `component_name` also come from the release component                   |
| F5 — `git_ref` == `version` identity / "not a git ref" not stated |     -5 | Both vars are `release_comp.version`; the quirk (a version in a `git_ref` slot) is not surfaced        |
| F6 — `apply_config` upper-cases env keys; not mentioned (D10)     |     -5 | `set_config` does `key.upper()`; a fidelity detail the port must reproduce                             |
| F7 — `find_path_relative_to` "from the directory" imprecision     |     -2 | `hint` is the `container.yaml` _file_, not its directory; first `joinpath` is a no-op miss             |
| **Total**                                                         | **63** |                                                                                                        |

Interpretation: 63 → "significant issues; must address before proceeding." The
score is driven by the two latent-bug omissions (F1, F2) and the dead-field gap
(F3), not by any unsoundness in the design's model. With F1–F4 addressed the
document is comfortably in the 85+ "acceptable" band.

## Findings (ordered by severity)

### F1 — `finish` never checks the buildah push return code (HIGH)

**Design claim.** Section "Buildah commit / push / sign sequence" (lines
131–144) narrates a clean five-step sequence: set annotations →
`commit --squash` → resolve creds → `push --digestfile [--creds] <uri>` → read
digest → optional `async_sign`. Step 4 reads "read the pushed digest back" with
no caveat.

**What the code shows.** `BuildahContainer.finish` (`utils/buildah.py:262-297`):

- The commit rc **is** checked (`utils/buildah.py:228`).
- After the push, there is **no `if rc != 0:` check**. `_buildah_run` returns
  `(rc, stdout, stderr)` and only _logs_ on non-zero — it does not raise
  (`utils/buildah.py:84-87`). So a failed push raises no exception.
- `finish` then opens the digest file the failed push never wrote and reads an
  empty/partial `image_digest` (`utils/buildah.py:265-266`), logging "pushed".
- `if not sign_with_transit: return` (`utils/buildah.py:278-280`) → for the
  no-signing path, **a failed push returns success**.
- With signing on, `image_digest` is `""`;
  `get_container_canonical_uri(desc, digest="")` takes the `:tag` branch because
  `""` is falsy (`utils/containers.py:48`), so cosign signs `<base>:<tag>` — the
  **wrong reference**, not `@<digest>`.
- The single `if rc != 0:` block at `utils/buildah.py:294-297` fires on the
  **stale push rc** but is labelled `"error signing image"` — misattributed, and
  unreachable on the no-signing path.

**Gap.** The design owns this sequence and presents it as correct. A port that
mirrors the prose will faithfully reproduce both a silent failed push reported
as success and a wrong-reference signing. This is exactly the "broken Python
behavior lurking" the review brief asks for.

**Recommended change.** Add an explicit note to the finish section: the Python
push rc is unchecked (a known defect); the Rust port MUST check the push exit
status immediately after the push and fail with `ContainerError` before reading
the digest, and MUST treat an empty digest as an error rather than signing the
`:tag` fallback. State this as a deliberate fix-on-port, or flag it as
carried-forward, but do not leave it unstated.

### F2 — `can_sign`/`sign` do not "require" creds; they raise an uncaught `ValueError` (MEDIUM)

**Design claim.** Section "Image signing" (line 161): "`async_sign` falls back
to unauthenticated if creds are absent; the sync path's `sign` requires them."

**What the code shows.** Both `can_sign` and `sign` route through
`_get_signing_params` (`images/signing.py:39-67`), which obtains registry creds
at lines 55-60 and catches **only `SecretsMgrError`**. But `registry_get_creds`
raises a **bare `ValueError`** when no matching secret is found
(`utils/secrets/registry.py:60-64`). That `ValueError` is therefore **not**
caught in `_get_signing_params` and propagates uncaught:

- `can_sign` (`images/signing.py:70-77`) catches only `SigningError`, so the
  `ValueError` **escapes `can_sign`** — a function typed `-> bool`. It does not
  return `False`; it raises. That is a contract violation, not a gate.
- `sign` (`images/signing.py:80-85`) likewise lets the `ValueError` escape (as a
  `ValueError`, not a `SigningError`).
- By contrast, `async_sign` explicitly catches `ValueError` and falls back to
  unauthenticated (`images/signing.py:136-140`) — the design's description of
  `async_sign` is correct.

**Reachability note (temper the severity).** In the only live caller of
`sign`/`can_sign` — `skopeo_copy` — dst creds are fetched at
`images/skopeo.py:77` (also catching only `SecretsMgrError`) _before_ `can_sign`
at `images/skopeo.py:102`, so the same `ValueError` class leaks from the earlier
call first. The contract bug in `can_sign`/`sign` is thus usually masked in
practice, but it remains a real typing/fidelity defect the port should not
blindly copy.

**Gap.** "requires them" reads as a clean precondition. The reality is an
asymmetry between `async_sign` (tolerant) and `sign`/`can_sign` (raises an
uncaught, mistyped error).

**Recommended change.** Restate as: "`async_sign` tolerates absent registry
creds (falls back to unauthenticated); the sync-path `sign`/`can_sign` do
**not** — `_get_signing_params` catches only `SecretsMgrError`, so a missing
secret surfaces as an uncaught `ValueError`, including out of `can_sign` (which
is typed `-> bool`). The port should normalise this to a typed result
(`SigningError`/`bool`)."

### F3 — `cond` is modelled but is dead in Python (MEDIUM)

**Design claim.** The `ContainerPackages` struct (line 47) lists
`required: Vec<ContainerPackagesEntry>` whose entry is
`{ section, packages, cond? }`, presenting `cond` as a live optional field.

**What the code shows.** `ContainerPackagesEntry.cond` (`containers/desc.py:62`)
is defined as `cond: str | None = Field(default= None)` and is **read nowhere**
in the codebase (a repo-wide search for the identifier returns only that
definition). `get_packages` (`containers/component.py:242-256`) iterates
`packages.required` and extends with `package_section.packages`, never
consulting `cond`.

**Gap.** The design models a field that does nothing. A port that mirrors the
struct will carry an inert `cond` with no indication it is parsed and then
ignored, inviting a future implementer to assume it is honoured.

**Recommended change.** Annotate `cond` in the struct/Fidelity notes as "parsed
for forward-compat but never evaluated (dead in Python)". Decide and record
whether the port keeps it (for schema tolerance) or drops it.

### F4 — `get_components` var-source enumeration is incomplete (LOW)

**Design claim.** Lines 80-81: the interpolation `vars` are assembled "from the
release component (version, sha1, repo_url) and the version descriptor (el,
distro)."

**What the code shows.** `build.py:115-123` assembles seven vars: `version`,
`el`, `git_ref`, `git_sha1`, `git_repo_url`, `component_name`, `distro`. Of
these, **five** come from the release component (`release_comp.version` twice —
as `version` and `git_ref`; `release_comp. sha1`; `release_comp.repo_url`;
`release_comp.name`) and **two** from the version descriptor (`el` =
`version_desc.el_version`, an `int`; `distro` = `version_desc.distro`). The
prose at line 81 names only three release-comp vars and omits `git_ref` and
`component_name` entirely.

**Gap.** The header list at lines 30-31 is correct (seven names), but the body
breakdown under-enumerates the release-component contributions.

**Recommended change.** Correct line 81 to: "from the release component
(`version`, `git_ref`, `git_sha1`/`sha1`, `git_repo_url`/`repo_url`,
`component_name`/`name`) and the version descriptor (`el` = `el_version`,
`distro`)." Note `el` is an `int`, so `{el}` renders its decimal form on
template interpolation.

### F5 — `git_ref` is identical to `version` and is a version, not a ref (LOW)

**Design claim.** Lines 30-31 list `git_ref` as a distinct interpolation var;
the brief flags that `git_ref` is the release component _version_, not a git
ref.

**What the code shows.** `build.py:116` sets `"version": release_comp. version`
and `build.py:118` sets `"git_ref": release_comp.version` — the **same value**.
The `git_ref` slot therefore carries a version string, and `version` and
`git_ref` are always equal.

**Gap.** The design treats them as two independent vars without noting they are
the same value or that `git_ref` is misleadingly named (it is not a git ref). A
`container.yaml` author or the port author could reasonably expect `git_ref` to
differ from `version`.

**Recommended change.** Add one line: "`git_ref` is set to the release
component's `version` (identical to the `version` var); despite the name it is
not a git ref. The port should reproduce the value but the naming is a known
wart."

### F6 — `apply_config` upper-cases environment keys (LOW)

**Design claim.** Line 124: "`apply_config`: `buildah config` the descriptor's
env / labels / annotations (skipped if all empty)."

**What the code shows.** `BuildahContainer.set_config`
(`utils/buildah.py:121-123`) emits env as `--env {key.upper()}={value}` — it
**upper-cases** each env key. Labels and annotations are passed through
verbatim.

**Gap.** A fidelity detail the design does not mention. A naive port that passes
env keys through unchanged would diverge from the committed image's environment
block.

**Recommended change.** Note in the `apply_config` bullet (or Fidelity notes)
that env keys are upper-cased before `buildah config --env`.

### F7 — `find_path_relative_to` starts at the file, not its directory (INFO)

**Design claim.** Lines 126-129: `_run_script` and file-repo resolution use
`find_path_relative_to(name, hint, root)`, described as "walk up from the
`container.yaml`'s directory to the `containers/` root."

**What the code shows.** `find_path_relative_to`
(`containers/__init__.py:29-39`) starts at `p = hint` and does
`p.joinpath(name)` before stepping to `p.parent`. The `hint` passed is
`self.container_file_path`, which is the `container.yaml` **file** path
(`component.py:157`, `component.py:261-265`), not a directory. The first
iteration therefore tests `<…>/container.yaml/<name>`, which never exists, then
proceeds to the parent directory and upward to `root`.

**Gap.** Functionally equivalent to the design's wording (the bogus first join
always misses), but the loop boundary the port reproduces starts one level
"inside" a file path. Worth a precise word so the port's `root`/`hint` boundary
matches exactly.

**Recommended change.** Optionally tighten to "walk up from the `container.yaml`
**path** (the first join onto the file path is a harmless miss) to the
`containers/` root." Low priority; can be dropped if space is tight.

## Items verified correct (no finding)

These were scrutinised and match the source; recorded so the next reviewer need
not re-derive them.

- **Repos union by `source` scheme.** `repo_discriminator`
  (`containers/repos.py:35-47`) dispatches on the `source` prefix: `file://` →
  file, `http(s)://` → url, `copr://` → copr; empty or unrecognised source
  raises (a bare `pydantic.ValidationError()`). The design's
  custom-`Deserialize`-by-scheme model with no added tag faithfully reproduces
  this, and the "unrecognised scheme errors" claim is accurate.
- **Per-variant fields.** `ContainerFileRepository` and `ContainerURLRepository`
  both declare `dest` (`containers/repos.py:84-85, 121-122`);
  `ContainerCOPRRepository` has no `dest` (`containers/repos.py:158`). The
  design's enum (file/url have `dest`, COPR does not) is correct.
- **Per-variant `install`.** file/url copy a resolved `.repo` source into the
  container at `dest` via `_install_path`
  (`containers/repos.py:67-91, 124-128`); COPR runs `dnf copr enable -y <spec>`
  (`containers/repos.py:160-177`). Matches the design.
- **Descriptor selection.** `_get_container_desc` / `_find_container_root_path`
  (`containers/component.py:31-138`) ranks candidate `container.yaml` files by a
  per-part match over `(prefix, major, minor, patch, suffix)` with the bare root
  file as rank-0 fallback, no cross-file merge, after `normalize_version`. The
  design's summary is accurate.
- **Build sequence.** `build()` (`containers/build.py:46-84`) runs
  get-components → `buildah_new_container` → `apply_pre` → `install_packages` →
  `apply_post` → `apply_config`, strictly sequentially, each on the one shared
  container. Missing-from-release-build → skip with a warning
  (`build.py:109-111`); empty result → `ContainerError` (`build.py:146-149`).
  Matches.
- **Required-only single `dnf`.** `get_packages(optional=False)` collects only
  `packages.required` (`build.py:167-175`, `component.py:242-256`); the
  aggregate install is one
  `dnf install -y --setopt=install_weak_deps=False --setopt=skip_missing_names_on_install=False --enablerepo=crb`
  (`build.py:188-196`). The commented-out final `dnf update` is omitted
  (`build.py:208-219`). All accurate.
- **`apply_pre` `dnf` distinction.** The per-component `apply_pre` `dnf` uses
  **only** `--setopt=install_weak_deps=False` (no `skip_missing`, no `crb`)
  (`component.py:203-209`), distinct from the aggregate. The design correctly
  preserves this distinction (line 117 vs line 88) — do not flatten it.
- **`finish` annotations and creds.** OCI annotations set before commit
  (`utils/buildah.py:198-204`); `--creds` is a `Password` only when both
  username and password are present (`utils/buildah.py:255-260`); `ValueError`
  from `registry_creds` falls back to unauthenticated with a warning
  (`utils/buildah.py:241-244`). Matches (modulo F1's push-rc gap).
- **cosign Vault via environment + redaction.** `VAULT_ADDR`/`VAULT_TOKEN`/
  `TRANSIT_SECRET_ENGINE_PATH` are set in `extra_env`, never in argv
  (`images/signing.py:170-180`). **Verified:** `async_run_cmd`
  (`utils/__init__.py:206-224`) logs only `_sanitize_cmd(cmd)` and the raw
  `cmd`, and **never logs `env`/`extra_env`** — so `VAULT_TOKEN` does not reach
  the logs via the command path. The design's redaction claim holds; the port
  must likewise never log `extra_env`.
- **Sync is standalone.** `sync_image` (`images/sync.py:23-87`) is not invoked
  from `build.py` or `finish`; its only signing path is via `skopeo_copy` →
  `sign`. The "not part of the build pipeline" claim is correct.
  `MissingTagError` on a missing source tag, skip-if-dst-tag-exists (unless
  `force`), `dry_run` skips the copy — all match.
- **URI helpers.** `get_container_image_base_uri` /
  `get_container_canonical_uri` (`utils/containers.py:21-48`) behave as
  described (note the `digest` falsy branch is the mechanism behind F1).
- **template-then-parse.** `ContainerDescriptor.load`
  (`containers/desc.py:82-104`) reads the file, `.format(**vars)`-es when vars
  are present, then `yaml.safe_load` + `model_validate`. Matches; the
  golden-test recommendation is appropriate.
