# cbscore-rs — design documentation set

This directory holds the design corpus for the **Rust port of `cbscore`**, the
core build library of the CES Build System (CBS). The port exists so that
`cbsd-worker` can run builds by calling the build runner **directly in-process**
(linking a Rust `cbscore` library) instead of shelling out to the Python tool
through `cbsd-rs/scripts/cbscore-wrapper.py`.

Build isolation is unchanged: every build still runs inside a podman builder
container. What changes is the orchestrator — an in-process Rust `runner::run`
instead of a `python3` subprocess.

## What `cbscore` does (two-phase build)

- **Host side** (`runner::run`): read the version descriptor, aggregate
  component directories, marshal secrets and a path-rewritten config to temp
  files, then `podman run` the builder image (`desc.distro`) with everything
  mounted, streaming output back.
- **In-container side** (`cbsbuild runner build` → `Builder.run`): prepare the
  toolchain, build RPMs per component, optionally GPG-sign, upload RPMs +
  release descriptors to S3, build/push the container image, optionally
  cosign-transit-sign, and write `build-report.json`.
- The report crosses back as a file on the scratch mount; the host reads it and
  returns it natively to the caller.

## Crate layout (target)

Three new members of the `cbsd-rs/` Cargo workspace:

| Crate           | Role                                                                       |
| --------------- | -------------------------------------------------------------------------- |
| `cbscore-types` | Zero-IO wire types (version/container/release/image descriptors, reports). |
| `cbscore`       | The library: subprocess/tools, config/secrets/Vault, S3, builder, runner.  |
| `cbsbuild`      | The thin `clap` CLI binary, built static-musl, mounted into the builder.   |

Dependency direction: `cbscore-types` ← `cbscore` ← `cbsbuild`, and the existing
`cbsd-worker` ← `cbscore`. Nothing depends on `cbsbuild`.

## How this doc set is organized

Documents follow the `seq-docs-convention` (`cbsd-rs-docs` skill):

- **`design/`** — `<seq>-<ts>-<title>.md`, the authoritative subsystem designs.
- **`reviews/`** — `<seq>-<ts>-<type>-<title>-v<N>.md`, the adversarial reviews
  (each design verified against the Python source, with a confidence score).
- **`plans/`** — phased, capability-sliced implementation plans (authored after
  the design review gate).
- **`000-…-design-review-v1.md`** — the standalone adversarial review of the
  original (reference) design corpus that seeded this port (verdict, blockers
  B1/B2, HIGH gaps H1–H5, MEDIUM M1–M3).
- **`ROADMAP.md`** — forward-looking items the port reproduces faithfully now
  and defers a decision on (see also `cbsd-rs/docs/ROADMAP.md`).

### Design index

| #   | Title                                        | Owns (single source of truth)                                                                                                                                                                                                                  |
| --- | -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 001 | Architecture & implementation spine          | Crate layout, B1 binary provisioning, B2 failure isolation, schema policy, the capability commit map, correctness invariants. **Read first.**                                                                                                  |
| 002 | Wire types, errors & schema versioning       | Descriptor/report types, kebab/snake casing, discriminated unions, `schema-version`/`report_version`, error taxonomy.                                                                                                                          |
| 003 | Subprocess, redaction & shell tools          | `run_cmd` primitive, `SecureArg`/`CmdArg` redaction, git/podman/buildah/skopeo wrappers.                                                                                                                                                       |
| 004 | Configuration, secrets & Vault               | Config/Secrets/Vault formats, `SecretsMgr` resolution, the Vault client. (Operator format reference below.)                                                                                                                                    |
| 005 | Storage: S3 & releases                       | `aws-sdk-s3` client (custom endpoint, path-style), release read/write.                                                                                                                                                                         |
| 006 | Versions                                     | Version types, parse helpers, `version_create_helper`, UUIDv7, `versions list`.                                                                                                                                                                |
| 007 | Builder pipeline                             | `Builder.run` orchestration, component scripts, stages (prepare/rpmbuild/sign/upload).                                                                                                                                                         |
| 008 | Containers & images                          | `ContainerBuilder`, buildah finish/push/sign, cosign signing, image descriptors.                                                                                                                                                               |
| 009 | Runner & two-phase execution                 | Host `runner::run`, binary mount, mount table, cancellation, build-report round-trip.                                                                                                                                                          |
| 010 | CLI surface (`cbsbuild`)                     | The clap command tree, the complete per-subcommand flag table, exit codes.                                                                                                                                                                     |
| 011 | Worker integration                           | The in-process cutover: `cbsd-worker` linking `runner::run`, panic isolation, cancellation.                                                                                                                                                    |
| 012 | Static-musl acceptance & distro independence | **Extends 001:** the B1 acceptance gate is the distro-independent link-time staticness check (`ldd`/`file`); the runtime smoke run uses a representative EL image, not Rocky 9 as canonical. Refines 001's build-target section + invariant 7. |

### Capability / milestone map

001 owns the authoritative map. In brief: **M0** bootstrap (`C0`: static
`cbsbuild` runs as PID 1); **M1** `versions create` (`C1`); **M2** build,
decomposed by working increment (`C2` keystone end-to-end → `C3` RPMs → `C4`
sign+S3 → `C5` image push → `C6` full-parity reuse/skip/transit); **M3**
`versions list` (`C7`); **M4** worker cutover (`C8`); **M5** deferred
configurable version-descriptor location (`C9`).

## Settled decisions

1. **Reimplement fresh** — the reference corpus is consulted, not merged.
2. **In-process worker integration** — panic isolation at the tokio task
   boundary via `JoinError::is_panic()` (not `catch_unwind`); keep
   `panic = "unwind"`; cancellation via an explicit `CancellationToken` → runner
   `podman stop` (no async `Drop`).
3. **Static-musl `cbsbuild`** (`x86_64-unknown-linux-musl`) shipped into the
   worker image and mounted by explicit path (never "self").
4. **Pragmatic schema policy** — additive, backward-compatible changes do not
   bump; only cbscore-owned formats carry `schema-version` (the build report
   keeps `report_version`; convergence is roadmapped).
5. **`config init` dropped** — config is hand-authored; the formats are
   documented below.
6. **Git secrets gain an explicit `type:` tag** — the one operator-visible
   format change (see below).
7. **UUIDv7 auto-versions** are in from M1.

## "Broken Python" the port fixes (rather than reproduces)

Discovered while verifying each design against the source; each is fixed in the
port with a clear contract:

- `versions list` called `list_releases` with the wrong arity (006).
- `get_image_desc`'s raw-string filter never interpolates, so it always reports
  "missing" (006).
- `build --force` is a no-op when a full release already exists (007).
- `buildah` push return code is unchecked (008).
- `can_sign` can leak a `ValueError` (008).
- The runner reads the build report before the rc check, then **discards** it on
  failure (009) — the port carries the partial report on the failure path.
- The runner's temp **secrets file (plaintext creds)** is leaked on the success
  and `PodmanError` paths (009) — the port guards every temp file with RAII.
- `cmd_build` marshals a temp secrets file that `runner()` never uses — a
  pointless plaintext-credentials write (010); the in-process worker marshals no
  secrets at all (the runner owns it).

Faithfully-reproduced quirks awaiting a deliberate decision are in `ROADMAP.md`.

---

## Operator configuration reference

Because `config init` is not ported (decision 5), operators **hand-author**
three YAML files. This section is their format reference; design 004 is the
authoritative specification. All three are cbscore-owned and carry a
`schema-version` (kebab-cased; **absent → 1**). YAML is primary; JSON is
accepted by file suffix. Field names are **kebab-case**.

### `cbs-build.config.yaml` — main config

```yaml
schema-version: 1
paths:
  components: # one or more directories holding component definitions
    - /cbs/components
  scratch: /cbs/scratch
  scratch-containers: /var/lib/containers
  ccache: /cbs/ccache # optional
storage: # optional; the runner degrades gracefully when absent
  s3:
    url: https://s3.example.com
    artifacts: { bucket: ces-artifacts, loc: rpms }
    releases: { bucket: ces-releases, loc: releases }
  registry:
    url: harbor.clyso.com
signing: # optional; values are secret IDs resolved via the secrets file
  gpg: ces-gpg-key
  transit: ces-transit-key
logging: # optional
  log-file: /cbs/logs/cbs-build.log
secrets: # paths to secrets files (merged in order; later overrides earlier)
  - /cbs/config/secrets.yaml
vault: /cbs/config/vault.yaml # optional; path to the vault config file
```

`paths` is required; `storage`, `signing`, `logging`, `secrets`, and `vault` are
optional. `signing.gpg` / `signing.transit` are **secret IDs** — the keys looked
up in the `sign` map of the secrets file.

### `cbs-build.vault.yaml` — Vault config (optional)

Only needed when any secret is Vault-backed. Configure exactly one auth block;
the selection order when several are present is **AppRole → userpass → token**.
The KV v2 mount is fixed at `ces-kv`.

```yaml
schema-version: 1
vault-addr: https://vault.example.com
auth-approle: # one of these three blocks
  role-id: <role-id>
  secret-id: <secret-id>
# auth-user:
#   username: <user>
#   password: <pass>
# auth-token: <token>
```

### `secrets.yaml` — credentials

Four maps, each keyed by the resource the credential applies to. **`storage`**
and **`sign`** are looked up by **exact** key/id; **`git`** and **`registry`**
by **longest-prefix URI match**. Every entry carries `creds: plain | vault`; a
`vault` entry additionally carries `key`. For the git, storage, registry, and
gpg vault variants `key` is the **`ces-kv` path read** via the Vault client; the
one exception is the `transit` signing variant, whose `key` is the Vault
**transit key name** used directly in the cosign `hashivault://<key>` reference
(it is _not_ a KV-v2 read) with `mount` naming the transit engine. The
`(creds, type)` pair selects the variant:

| Family     | Discriminators       | Type values                                                                                |
| ---------- | -------------------- | ------------------------------------------------------------------------------------------ |
| `storage`  | `creds` + `type: s3` | `s3`                                                                                       |
| `sign`     | `creds` + `type`     | `gpg-armor-key` (plain), `gpg-single-key`, `gpg-pvt-key`, `gpg-pub-key`, `transit` (vault) |
| `registry` | `creds` only         | _(no `type` field)_                                                                        |
| `git`      | `creds` + `type`     | `ssh`, `token` (plain only), `https`                                                       |

> **Git `type:` is the one format change vs Python.** Python discriminated git
> secrets by field shape; the port requires an explicit
> `type: ssh | token | https`. Converting an existing file means adding one
> `type:` line per git entry. `token` is plain-only (there is no `vault-token`).

```yaml
schema-version: 1
git:
  github.com/ceph: # longest-prefix match against the repo URL
    creds: plain
    type: ssh
    username: git
    ssh-key: |
      -----BEGIN OPENSSH PRIVATE KEY-----
      ...
  gitlab.example.com:
    creds: plain
    type: token
    username: oauth2
    token: <token>
  # vault-backed HTTPS example:
  # git.internal.example.com:
  #   creds: vault
  #   type: https
  #   key: git/internal # read from ces-kv
storage:
  https://s3.example.com: # exact key (matches storage.s3.url)
    creds: plain
    type: s3
    access-id: <access-key-id>
    secret-id: <secret-access-key>
sign:
  ces-gpg-key: # exact id (matches signing.gpg)
    creds: plain
    type: gpg-armor-key
    email: build@clyso.com
    private-key: |
      -----BEGIN PGP PRIVATE KEY BLOCK-----
      ...
    # public-key / passphrase optional
  ces-transit-key: # exact id (matches signing.transit)
    creds: vault
    type: transit
    key: ces # transit key name (cosign uses hashivault://<key>)
    mount: transit # transit engine mount
registry:
  harbor.clyso.com: # longest-prefix match against the registry URL
    creds: plain
    username: <user>
    password: <pass>
    address: harbor.clyso.com
```

Per-family field shapes (kebab-cased; `vault` variants add `key`):

- **storage**: `{access-id, secret-id}` (+ `type: s3`).
- **sign**: `gpg-armor-key` (plain)
  `{private-key, public-key?, passphrase?, email}`; `gpg-single-key` (vault)
  `{private-key, public-key?, passphrase?, email}`; `gpg-pvt-key` (vault)
  `{private-key, passphrase?, email}`; `gpg-pub-key` (vault)
  `{public-key, email}`; `transit` (vault) `{mount}`.
- **registry**: `{username, password, address}`.
- **git**: `ssh` `{ssh-key, username}`; `token` `{token, username}` (plain
  only); `https` `{username, password}`.
