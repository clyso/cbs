# 008 — Containers & images

This is the reference design for the container-image assembly and image
operations of the `cbscore` library: the `ContainerBuilder` orchestrator, the
per-component `ComponentContainer` handler, the `ContainerDescriptor`
(`container.yaml`) type and its repos union, the buildah commit/push/sign
sequence, cosign transit image signing, and the standalone image sync. It owns
these as the single source of truth. Read 002 for
`VersionDescriptor`/`ReleaseDesc`, 003 for the buildah & skopeo wrappers and
`SecureArg`, 004 for `SecretsMgr`, and 007 for the builder orchestration that
invokes `ContainerBuilder`.

Source of truth: the `containers/` modules — `build.py`, `component.py`,
`desc.py`, `repos.py`, `__init__.py` — and the `images/` modules — `signing.py`,
`sync.py`, `skopeo.py` — plus `utils/containers.py` and the
`BuildahContainer.finish` sequence in `utils/buildah.py`.

This stage runs **inside the builder container** (007 invokes
`ContainerBuilder(...).build().finish(...)` at its container-image step). The
buildah and skopeo subprocess **wrappers** are owned by 003; this document owns
how they are composed.

## `ContainerDescriptor` (`container.yaml`)

Source: `containers/desc.py`. An **external, repo-authored, unversioned** input
(002 deferred it here): a `container.yaml` authored under a component's
`containers/` tree, read by cbscore — no `schema_version`. It is loaded YAML
with **string-template interpolation applied before parse**: the file text is
`.format(**vars)`-ed, then parsed. The `vars` (assembled by
`ContainerBuilder.get_components`, below) are `version`, `el`, `git_ref`,
`git_sha1`, `git_repo_url`, `component_name`, `distro`.

```rust
struct ContainerDescriptor {
    config: Option<ContainerConfig>,        // { env, labels, annotations: maps }
    pre: ContainerPre,
    packages: ContainerPackages,
    #[serde(default)] post: Vec<ContainerScript>,   // { name, run }
}
struct ContainerPre {
    #[serde(default)] keys: Vec<String>,            // GPG key URLs (rpm --import)
    #[serde(default)] packages: Vec<String>,        // dnf names or http(s) RPM URLs
    #[serde(default)] repos: Vec<ContainerRepo>,    // discriminated union, below
    #[serde(default)] scripts: Vec<ContainerScript>,
}
struct ContainerPackages {
    #[serde(default)] required: Vec<ContainerPackagesEntry>,  // { section, packages, cond? }
    #[serde(default)] optional: Vec<ContainerPackagesEntry>,
}
```

The **repos union** (`pre.repos`) is discriminated by the `source` URI
**scheme**, not a tag: `file://` → file, `http(s)://` → URL, `copr://` → COPR
(`repo_discriminator`). The Rust port models it as a custom `Deserialize` that
reads `source` and dispatches — **no added tag**, since the scheme is intrinsic
to `source` and unambiguous. (Contrast the git secrets of 004, whose Python
discrimination was by ambiguous field _shape_, warranting an explicit `type:`;
here the discriminator is a clean prefix on a field that is always present.)

```rust
enum ContainerRepo {
    File { name, source, dest },   // source "file://<path>"
    Url  { name, source, dest },   // source "http(s)://..."
    Copr { name, source },         // source "copr://<spec>"  (no dest)
}
```

`template-then-parse` and the discriminated repos union are the two
container-descriptor parsing concerns; a golden test pins both.

## `ContainerBuilder` orchestration

Source: `containers/build.py`. Constructed with the `VersionDescriptor`, the
`ReleaseDesc`, and the loaded `CoreComponent` map (007). `build()` runs,
**strictly sequentially**:

1. `get_components()` — for the `x86_64` release build (FIXME: arch should come
   from the descriptor; the port keeps single-arch), build a
   `ComponentContainer` per version-descriptor component that is present in the
   release build, assembling the interpolation `vars`: `version`, `git_ref`, and
   `component_name` from the release component's `version`/`name`, `git_sha1`
   from its `sha1`, `git_repo_url` from its `repo_url`, and `el`/`distro` from
   the version descriptor. **Note**: `git_ref` is set to the component's
   _version string_, not an actual git ref — a Python quirk the port reproduces.
   A component missing from the release build is skipped with a warning; an
   empty result is a `ContainerError`.
2. `buildah_new_container(desc)` (003) — `buildah from <distro>`.
3. `apply_pre(components)` — per component, in order.
4. `install_packages(components)` — collect the **required** packages from all
   components (optional are ignored today, a TODO) and install them in **one**
   `dnf install -y --setopt=install_weak_deps=False --setopt=skip_missing_names_on_install=False --enablerepo=crb <pkgs>`.
5. `apply_post(components)` — per component, in order. (A "final `dnf update`"
   is commented-out in Python with a FIXME about breaking pinned packages; the
   port omits it, matching current behavior.)
6. `apply_config(components)` — per component, in order.

There is **no per-component parallelism** here — every step mutates the one
shared buildah container, so the ordering is sequential by design (unlike the
builder stages of 007).

`finish(secrets, sign_with_transit)` delegates to the buildah finish sequence
(below).

## `ComponentContainer`

Source: `containers/component.py`. Wraps one component's selected
`container.yaml`.

- **Descriptor selection** (`_get_container_desc`): under the component's
  `containers/` directory, find the `container.yaml` whose enclosing directory's
  version name matches the build version **most closely** — a per-part rank over
  `(prefix, major, minor, patch, suffix)`, with the bare root `container.yaml`
  as rank-0 fallback (no merging across files). This version-proximity selection
  is intricate and ported faithfully (golden test). The version is
  `normalize_version`-d (006) first.
- **`apply_pre(container)`**, in order: run each `pre.scripts` entry (copy the
  script into the container at `/<name>`, `run`, then `rm -f`); import each
  `pre.keys` URL via `rpm --import`; split `pre.packages` into dnf names vs
  `http(s)://` RPM URLs, install the dnf set in one
  `dnf install -y --setopt=install_weak_deps=False` and each URL via `rpm -Uvh`;
  install each `pre.repos` entry (file/URL → copy a `.repo` file into the
  container; COPR → `dnf copr enable -y <spec>`).
- **`get_packages(optional=false)`**: the union of `packages.required` section
  package lists (optional ignored, a TODO).
- **`apply_post(container)`**: run each `post` script (same copy/run/rm).
- **`apply_config(container)`**: `buildah config` the descriptor's env / labels
  / annotations (skipped if all empty). Env keys are **upper-cased**
  (`buildah.py:123`, emitted as `KEY=value`); the port reproduces this.

`_run_script` and the file-repo source resolution use
`find_path_relative_to(name, hint, root)` (`containers/__init__.py`): starting
at the `container.yaml`'s directory and walking up to the `containers/` root,
return the first existing `<dir>/<name>` (file or directory), else `None`. The
port reproduces this lookup.

## Buildah commit / push / sign sequence

Source: `BuildahContainer.finish` (`utils/buildah.py`; 003 owns the wrapper,
this document owns the sequence). On `finish`:

1. Set OCI annotations (`org.opencontainers.image.created` / `url` / `version`).
2. `buildah commit --squash` to the **canonical URI**
   (`get_container_canonical_uri` = `<registry>/<name>:<tag>`).
3. Resolve registry credentials via `secrets.registry_creds(base_uri)` (004); on
   `ValueError` (none configured) fall back to unauthenticated with a warning.
4. `buildah push --digestfile <tmp> [--creds <Password>] <uri>` — the `--creds`
   value is a `Password` (003); read the pushed digest back.
5. If a transit id is set, `async_sign(<uri>@<digest>, secrets, transit)`
   (below).

**Push error handling is fixed (Python bug).** Python's `finish` never checks
the push exit code where it should: `_buildah_run` only logs a non-zero `rc`
(`utils/buildah.py:84-87`), and `finish`'s only `rc != 0` guard sits **after**
the sign step and is mislabeled "error signing" (`buildah.py:294-297`). So on
the no-signing path a failed push reports success, and on the signing path a
failed push yields an empty digest, making
`get_container_canonical_uri(desc, digest="")` fall through to the `:tag` branch
— cosign then signs the wrong reference. The port fixes this: check the push
`rc` **immediately** and fail with a push-specific error; require a non-empty
digest before signing; sign the `@<digest>` reference.

## Image signing (cosign + Vault transit)

Source: `images/signing.py`. `async_sign(img, secrets, transit)` signs an image
with a Vault **transit** key via cosign. Preconditions (`can_sign` / the
precondition block): a Vault must be configured and the named transit key must
exist, else `SigningError`.

```
cosign sign --key=hashivault://<transit_key> \
  [--registry-username <PasswordArg> --registry-password <PasswordArg>] \
  --tlog-upload=false --upload=true <img@digest>
```

- The registry credentials are `PasswordArg`s (003), so they are redacted in
  logs. `async_sign` falls back to unauthenticated if creds are absent; the sync
  path's `sign` requires them. **`can_sign` is hardened (Python bug)**: Python's
  `can_sign` (typed `-> bool`) catches only `SecretsMgrError`, but
  `registry_get_creds` raises a bare `ValueError` on missing creds
  (`registry.py`), so `can_sign` can _raise_ rather than return `False`. The
  port catches that case and returns `False`, honoring the `-> bool` contract.
- cosign reads Vault via **environment**, not the command line: `VAULT_ADDR`,
  `VAULT_TOKEN` (pulled from the Vault client), and `TRANSIT_SECRET_ENGINE_PATH`
  (the transit mount, from `secrets.transit(id)` → `(mount, key)`, 004).
  **Redaction note**: the subprocess primitive (003) logs the redacted
  _command_, not the environment, so `VAULT_TOKEN` is not logged via the command
  path; the port must likewise never log `extra_env`. The token travels in the
  child's environment (not its argv), which is the appropriate place for it.

## Image sync (standalone)

Source: `images/sync.py`.
`sync_image(src, dst, dst_registry, secrets, transit, force, dry_run)` copies an
image between registries: it lists src/dst tags (`skopeo_get_tags`, 003), errors
on a missing source tag, skips when the dst tag already exists (unless `force`),
then `skopeo_copy` + sign (`dry_run` skips the copy). This is **not part of the
build pipeline** — it is a standalone image operation (consumed by the
`advanced`/sync surface, 010, or future callers); it is specified here because
it lives in `images/` and reuses the skopeo wrapper and cosign signing.
`MissingTagError` is its dedicated error.

## URI helpers

Source: `utils/containers.py`.
`get_container_image_base_uri(desc | str) -> "<registry>/<name>"` (from a
`VersionDescriptor`, or by stripping `:tag`/`@digest` from a string).
`get_container_canonical_uri(desc, digest?) -> base + ("@<digest>" | ":<tag>")`.
Pure helpers; they live with this subsystem.

## Errors

Per 002, these IO-layer errors live with this subsystem (in `cbscore`):
`ContainerError` (descriptor load, repo install, any build-stage failure),
`SigningError` (cosign/transit signing), and the skopeo image errors already
noted in 003 (`SkopeoError`, `ImageNotFound`, `UnknownRepository`, plus
`MissingTagError` for sync). Lower-level errors (`BuildahError`,
`SecretsMgrError`, `CommandError`) are wrapped with context.

## Fidelity notes

- **`container.yaml` is unversioned**, repo-authored, and **template-then-
  parsed** (`{version}`/`{el}`/`{git_sha1}`/… interpolated before YAML parse).
- **Repos union by `source` scheme** (file/URL/COPR) via a custom `Deserialize`
  — no added tag (contrast the git-secrets tag in 004).
- **Sequential build** — `apply_pre`/`install_packages`/`apply_post`/
  `apply_config` run per component in order on the one shared container; no
  fan-out.
- **Required-only packages** — `optional` package sections are ignored today (a
  TODO); the required sections install in a single `dnf` invocation with `crb`
  enabled and weak deps off. The `cond` field on a package entry is **parsed but
  never evaluated** in Python (dead config today); the port keeps it for
  parse-compatibility and likewise does not evaluate it.
- **Commented-out final `dnf update`** in `apply_post` is omitted (matching
  current behavior; the Python FIXME notes it could break pinned packages).
- **Single arch** (`x86_64`) — the same FIXME as 007, carried as a known
  limitation.
- **Push/sign credential handling** — `buildah push --creds` is a `Password`,
  cosign registry creds are `PasswordArg`s, and `VAULT_TOKEN`/transit reach
  cosign via the environment (never the argv, never logged).
- **Push errors caught (Python bug fixed)** — the port checks the `buildah push`
  rc and requires a non-empty digest before signing; Python silently succeeds on
  a failed push and can sign the wrong reference.
- **`can_sign` hardened** — returns `False` on missing creds rather than leaking
  a `ValueError` (Python bug).
- **`git_ref` == version** — the container-descriptor `{git_ref}` var is the
  component version string, not a git ref (reproduced quirk).
- **Image sync is standalone**, not part of the build.

## Testing

- **Descriptor parse**: golden test of `container.yaml` template-then-parse with
  a representative `vars` set, and of the repos union (a `file://`, `http://`,
  and `copr://` entry each resolving to the right variant; an unrecognized
  scheme erroring).
- **Descriptor selection**: `_get_container_desc` version-proximity ranking over
  a nested `containers/` tree (exact > minor > major > root fallback; no
  cross-file merge).
- **Build sequence**: `build()` runs get-components → new-container → pre →
  packages (one `dnf install` of the required union) → post → config, in order;
  a component absent from the release build is skipped; an empty component set
  errors.
- **apply_pre**: scripts copy/run/rm; `keys` import; dnf-vs-URL package split;
  each repo variant installs correctly (file/URL copy a `.repo`; COPR enables).
- **finish**: commit-canonical-URI → push-with-digest → optional transit-sign;
  `--creds` redacted; no registry creds → unauthenticated with a warning; a
  **failed push errors** (not silent success), and signing requires a non-empty
  digest (signs `@<digest>`, never `:tag`).
- **Signing**: `can_sign` false without a Vault or transit key; `async_sign`
  builds the cosign command with `PasswordArg` creds and the Vault env; the
  token never appears in a captured log.
- **Sync**: skip when the dst tag exists (unless `force`); `dry_run` does not
  copy; a missing source tag errors.
