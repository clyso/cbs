# crt — Ceph Release Tool (v2)

`crt` builds and publishes **downstream Ceph releases** as cryptographically
verifiable artifacts. A release is a curated set of patches carried on top of an
upstream base ref; `crt` ingests those patches, lets you author release
metadata, **seals** the result into a signed, write-once manifest, and
**materializes** it into a linear git branch plus a self-verifying
`000-RELEASE/` bundle and an annotated tag. Anyone who later receives the branch
— as a clone, a tarball, or a ZIP — can verify it **offline** against a single
published OpenPGP public key.

It is the Rust reimplementation of the older Python release tool, living in
three crates of the `cbsd-rs/` workspace:

- **`crt-core`** — pure domain logic, no I/O: the manifest model, the risk
  rubric, RFC 8785 canonical JSON + sha256 digests, detached OpenPGP
  sign/verify, the deterministic CycloneDX SBOM, minijinja notes rendering, and
  the `source_tree_digest`.
- **`crt-store`** — content-addressed persistence over
  [`object_store`](https://docs.rs/object_store) (in-memory, local filesystem,
  or S3): patch blobs, patch metadata, mutable drafts, write-once sealed
  releases, and notes templates.
- **`crt`** — this crate: the `clap` CLI and the I/O edges (subprocess `git`,
  GitHub via `octocrab`, the signing key via HashiCorp Vault, the public key via
  `reqwest`).

## The release lifecycle

Everything `crt` does is one step of this pipeline:

```text
patch import         →  ingest patches into the content-addressed store
release new          →  open an empty draft on an upstream base ref
release add          →  curate it with patches + risk/notes metadata
release seal         →  sign and freeze into a write-once record
release materialize  →  emit the git branch + signed bundle + tag
verify               →  re-derive and check everything
```

1. **`patch import`** — pull patches from a local git range or a GitHub PR into
   the content-addressed store. Each patch is keyed by the sha256 of its bytes
   (its `blob_hash`).
2. **`release new`** — open an empty _draft_ for a release name, pinned to an
   upstream `base_ref`.
3. **`release add`** — attach imported patches to the draft as _entries_, each
   carrying its risk metadata and the public-facing notes copy.
4. **`release seal`** — freeze the draft into a signed, **write-once** release
   record. The canonical manifest is signed with the OpenPGP key fetched from
   Vault; the draft is then removed. A sealed release can never be re-sealed or
   mutated.
5. **`release materialize`** — replay the sealed release into a destination git
   repo: one commit per patch (`git am`, each commit tagged with a `Crt-Patch`
   trailer), then a signed `000-RELEASE/` verification bundle commit and an
   annotated tag carrying the manifest digest.
6. **`verify`** — re-derive and check everything, either offline from an
   extracted tree (`crt verify --tree`) or against the store and the git
   artifact (`crt release verify --repo`).

A release **name resolves to a channel by prefix** (e.g. `ces-v18.2.0` → channel
`ces`), and the channel supplies the branding stamped into the manifest at seal
time. See [Configuration](#configuration).

## Building

`crt` is part of the `cbsd-rs` Cargo workspace and builds with no database and
no network:

```bash
cd cbsd-rs
cargo build -p crt --release
# binary at cbsd-rs/target/release/crt
```

During development, run it straight from the workspace:

```bash
cd cbsd-rs
cargo run -p crt -- release list
```

The TLS and crypto stacks are pure-Rust (`rustls`, `pgp`) — there is no OpenSSL
or other C dependency to install. The only external runtime requirement is a
`git` binary on `PATH` (used by `patch import`, `release materialize`, and the
ref-conditional verify legs).

## Configuration

`crt` reads two YAML files. Non-secret settings live in **`crt.config.yaml`**;
credentials live in **`crt.secrets.yaml`**. Both default to the current
directory and can be relocated:

| Flag        | Env var       | Default              |
| ----------- | ------------- | -------------------- |
| `--config`  | `CRT_CONFIG`  | `./crt.config.yaml`  |
| `--secrets` | `CRT_SECRETS` | `./crt.secrets.yaml` |

Copy the checked-in examples to start:

```bash
cp crt.config.yaml.example  crt.config.yaml
cp crt.secrets.yaml.example crt.secrets.yaml
chmod 600 crt.secrets.yaml
```

Both files are git-ignored. Whenever `crt` reads the secrets file — for an S3
store, `release seal`, or `release materialize` — it warns if the file is group-
or world-readable.

### `crt.config.yaml` (non-secret)

```yaml
# The component these releases are built from.
component: ceph

# Store backend — exactly one of `local` or `s3`.
store:
  local: /var/lib/crt/store
  # s3:
  #   endpoint: https://s3.example.com
  #   region: us-east-1
  #   bucket: my-crt-bucket
  #   prefix: crt/

# Destination repo for `release materialize`. A logical identifier (e.g. a
# `clyso/ceph` slug); the actual local working copy is supplied with --repo.
# Optional.
destination_repo: clyso/ceph

# Allowed labels for `release add --component`. An empty list (or omitting the
# key) accepts any label.
risk_components: [rgw, rgw-multisite, dashboard, build, docs, other]

# Where the OpenPGP public key is published, used by `verify`. An http(s) URL
# or a local path; the --public-key flag overrides it. Optional.
public_key_url: https://download.clyso.com/crt/crt-pubkey.asc

# Namespaces → channels. A release name resolves to a channel by prefix, and
# the channel's branding is snapshotted into the manifest at seal time.
namespaces:
  clyso-enterprise:
    channels:
      ces:
        branding:
          display_name: Clyso Enterprise Storage
          blurb: Long-term-supported, hardened Ceph for enterprises.
          footer: © Clyso — https://clyso.com
```

**Name resolution.** A channel key `C` matches a release name `N` when `N == C`
or `N` starts with `C-`. The most specific (longest) match wins, so with
channels `ces` and `ces-lts`, `ces-lts-v1` resolves to `ces-lts` while `ces-v1`
resolves to `ces`. A name matching two channels equally is rejected as
ambiguous, as is a name matching none.

### `crt.secrets.yaml` (secret — `chmod 600`)

```yaml
# Required only when the store backend is `s3`.
s3:
  access_key_id: AKIAEXAMPLE
  secret_access_key: example-secret-key

# Required for `release seal` and `release materialize`, which sign with the
# OpenPGP key kept in HashiCorp Vault (KV v2). The secret at the path below must
# carry the armored private key in a `private-key` field, with an optional
# `passphrase` field — matching cbscore's GPGVaultPrivateKeySecret convention so
# one Vault secret can serve both tools.
vault:
  addr: https://vault.example.com
  token: s.exampletoken
  keys:
    gpg_signing_private: secret/data/crt/openpgp-signing-key
```

The signing key is fetched at sign time and **never persisted by `crt`**. The
configured Vault path may be written with or without the KV v2 `data` infix
(`secret/data/crt/key` or `secret/crt/key`).

## Commands

Global options (`--config`, `--secrets`) apply to every subcommand.

### `patch import` — ingest patches

```bash
# From a local commit range
crt patch import --repo /path/to/ceph.git --range v18.2.0..my-backports

# From a GitHub PR (head/base are fetched into --repo; patch bytes always come
# from a local `git format-patch`)
crt patch import --repo /path/to/ceph.git \
    --pr https://github.com/ceph/ceph/pull/12345
```

Pass `--github-token` (or set `GITHUB_TOKEN`) to raise GitHub API rate limits.
The git fetch is anonymous, so private-repo PRs are not supported. Each line of
output reports the patch's `blob_hash` and subject and whether it was newly
imported or already present; a warning is printed if a patch is byte-different
but _equivalent_ (same `patch_id`) to one already stored.

### `release new` — open a draft

```bash
crt release new ces-v18.2.0 --base-ref v18.2.0
```

Author name/email default to your `git config` identity; override with
`--author-name` / `--author-email`.

### `release add` — curate the draft

Attach one or more imported patches (by `blob_hash`) to the draft. The risk and
notes flags apply to **every** blob listed in the call:

```bash
crt release add ces-v18.2.0 <blob_hash> [<blob_hash>...] \
    --component rgw \
    --blast <cosmetic|availability|data-loss> \
    --conflict <clean|trivial|substantive> \
    --coverage <strong|partial|weak> \
    --category fix \
    --justification <cve|customer|engineering> \
    --ref CVE-2026-1234 --ref https://tracker.ceph.com/issues/000 \
    --public-summary "Fix a crash in the RGW frontend under load"
```

- `--component` is validated against `risk_components` from the config.
- `--blast`, `--conflict`, `--coverage`, and `--justification` feed the integer
  risk rubric.
- `--public-summary` is the copy rendered into the notes. **If you omit it,
  `$EDITOR` opens** so you can compose the public summary, behavior-change note,
  and upgrade note in one session. Explicit `--behavior-change` /
  `--upgrade-notes` flags take precedence over the editor's sections.
- `--internal <note>` is stored but **never rendered or materialized** — it is
  for your records only.
- `--visibility <public|private>` is recorded but inert in the MVP (defaults to
  `public`).

### `release seal` — sign and freeze

```bash
crt release seal ces-v18.2.0
```

Fetches the signing key from Vault, signs the canonical manifest, writes the
write-once release record, and deletes the draft. Re-sealing an existing release
is refused (and the key is never even fetched in that case).

### `release list` / `info` / `notes`

```bash
crt release list                # sealed releases, as namespace/channel/name
crt release info ces-v18.2.0    # the draft, or the sealed release if no draft
crt release notes ces-v18.2.0   # re-render notes from the sealed RenderSpec
```

### `release materialize` — build the git artifact

```bash
crt release materialize ces-v18.2.0 --repo /path/to/local/ceph.git
```

Builds the linear `release/ces-v18.2.0` branch in the destination repo: `git am`
each entry's patch in order (every commit carrying a `Crt-Patch` trailer), then
append the signed `000-RELEASE/` verification bundle commit and an annotated tag
carrying the manifest digest. Signing the bundle needs the Vault key, so this
command also reads the `vault` section.

- `--repo <path>` is the local working copy to build in; it overrides
  `destination_repo` from the config.
- `--out <dir>` additionally emits the loose `RELEASE-NOTES.md` and
  `sbom.cdx.json` artifacts there. The in-tree bundle is authoritative; this is
  an optional convenience emit.
- `--push` publishes the branch and tag to `origin` (atomically) after building
  them locally. Opt-in; requires push access to the destination remote.

### Verifying a release

`crt` offers two verification entry points; both need the OpenPGP public key
(`--public-key <path-or-url>`, or `public_key_url` from the config).

**Offline, from an extracted tree** — the primary trust path for anyone who
receives a tarball, ZIP, or clone. No store, no git, no network:

```bash
crt verify --tree /path/to/extracted/release \
    --public-key ./crt-pubkey.asc
```

It verifies `000-RELEASE/record.json.asc` against the public key, then
recomputes the `source_tree_digest` and every digest listed in the bundle.

**Against the store (and optionally the git repo)** — for the release author:

```bash
# Legs 0–2 (sealed manifest): signature, schema, cross-reference
crt release verify ces-v18.2.0

# Add the ref-conditional legs 3–4 when the release has been materialized
crt release verify ces-v18.2.0 --repo /path/to/local/ceph.git
```

With `--repo`, if the release's tag exists, the ref-conditional legs run over
the git artifact; otherwise legs 3–4 are reported _skipped_.

### Exit codes

| Code | Meaning                                                             |
| ---- | ------------------------------------------------------------------- |
| `0`  | All applicable verification legs passed.                            |
| `1`  | Operational error: no such release, a store/git/Vault failure, etc. |
| `2`  | Signature verification failed (leg 0); clap exits `2` on bad usage. |
| `3`  | Schema / cross-ref / tree / git / artifact check failed (legs 1–4). |

## The trust model

Verification is layered into numbered **legs**. The three entry points check
different things, so a leg number means different things across them — `crt`
prints the artifact in each leg's label (e.g. _"signature (sealed manifest)"_ vs
_"signature (000-RELEASE bundle)"_). What runs depends on what you have in hand.

**`crt verify --tree <dir>`** — offline, no store and no git. The trust path for
a tarball/ZIP/clone recipient:

- **Leg 0 — signature (000-RELEASE bundle):** the detached OpenPGP signature
  over the exact on-disk bytes of `record.json` verifies under the public key.
- **Leg 1 — schema (in-tree record):** `record.json` deserializes and its schema
  version is the one this build supports.
- **Leg 2 — source + bundle digests:** the recomputed `source_tree_digest`
  equals the record's, and every file in `000-RELEASE/` matches its listed
  digest — and that list is _exhaustive_, so no unsigned file can be smuggled
  in.

**`crt release verify <name>`** — the sealed manifest in the store (legs 0–2):

- **Leg 0 — signature (sealed manifest):** the manifest signature verifies.
- **Leg 1 — schema (sealed manifest):** the manifest schema version is
  supported.
- **Leg 2 — cross-reference (sealed manifest):** the recomputed manifest digest
  equals the stored integrity anchor, and every referenced patch blob and its
  metadata is present and consistent.
- **Legs 3–4** are reported _skipped_ — they need `--repo`.

**`crt release verify <name> --repo <path>`** — when the release's tag exists,
this adds the git-artifact legs. On top of the sealed legs 0–2 above, it checks
out the tag and runs the in-tree bundle legs 0–2 (the same three checks as
`verify --tree`), then:

- **Leg 2 — cross-reference (in-tree record):** the in-tree record
  back-references the sealed manifest digest, and its patch BOM is a faithful
  projection of the sealed entries (order, blob, `patch_id`, count).
- **Leg 3 — git anchoring:** each patch commit's `Crt-Patch` trailer names its
  blob, and the `patch_id` recomputed from the commit's diff (offset-invariant)
  equals the sealed entry's.
- **Leg 4 — artifact faithfulness:** the committed `sbom.cdx.json` and
  `RELEASE-NOTES.md` byte-match a re-derivation from the sealed manifest.

The bundle is **signed by construction**: `record.json.asc` signs the exact
on-disk bytes of `record.json`, and `record.json` carries an exhaustive list of
digests for every other file in `000-RELEASE/`. Tampering with any bundled file,
the source tree, or a materialized commit is caught by one of these legs.

> Verification checks out the materialized tag with `git worktree add --detach`
> (never `git archive`), so the bytes it hashes equal what a recipient sees in a
> plain clone or tarball — `git archive` would re-apply `.gitattributes` and
> falsely mismatch.

## Status and scope

`crt` v2 delivers the full release MVP: ingest → seal → materialize → verify,
with offline and git-anchored verification. Items deliberately **out of scope**
for the MVP and not yet built:

- A `crt` service + web UI (the visibility gateway).
- **Enforcement** of per-entry `visibility` (recorded today, but inert).
- A CycloneDX SBOM schema validator.
- Migration from the older Python `crt` store format.
- Signed git tags / cosign, and signing-key rotation.

See `cbsd-rs/docs/crt/` for the authoritative design documents, implementation
plans, and reviews.
