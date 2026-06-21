# CRT v2 — Concept

> **Status:** Concept (pre-design). This document fixes the problem, the
> decisions, and the architecture for a second iteration of the Ceph Release
> Tool. It precedes the numbered design documents under `crt/design/` and the
> plans under `crt/plans/`. It is a snapshot of intent, not a commitment to a
> specific commit sequence — that belongs in the plan documents, each approved
> before implementation.

## 1. Context & Problem

Clyso ships a downstream Ceph product defined as **a set of patches on top of an
upstream Ceph release**. The tool that manages those patches must satisfy six
properties:

1. **Immutable once published** — a released patch set cannot change afterward.
2. **Auditable** — we can state exactly which patches went into a release.
3. **Reusable** — a patch (e.g. a base downstream tunable) can appear in many
   releases without duplication.
4. **Selectively visible** — the release manager marks each patch public or
   private; Ceph's LGPL licensing permits keeping some patches private.
5. **CLI now, service later** — usable as a CLI today, but architected so a
   service + web UI is a natural extension, not a rewrite.
6. **Backed by storage we control** — an S3 store is available; any backend may
   be chosen.

### What the first iteration (`crt/`, Python) taught us

The existing `crt/` tool established a good **domain model** — rich patch
provenance (`author`, dates, `cherry_picked_from`, `related_to`, git
`patch-id`), patchset variants (GitHub PR / custom / single), and a
manifest/stage/release structure. We keep that model as conceptual input.

Its defining architectural choice, however, is **git-as-store**: the patches
repository is a git working tree (JSON + symlinks), one branch per release, with
immutability delivered as an operator-pushed git tag. Measured against the six
properties, this does not hold up:

- **Visibility is unimplemented**, and git has no sub-repository access control
  — tiered visibility would force N separate repos or a mediating service.
- **S3 is declared but unused**; storage is local-filesystem git only.
- **Immutability is "soft"** — procedural (a pushed tag), not verifiable.
- **Service-readiness is weak** — working trees, branch checkouts, and symlinks
  do not map onto a concurrent, multi-user service + web UI.
- **Producer and consumer are unwired** — `crt` writes patches one way; the
  `cbsbuild` builder discovers patches in `components/<comp>/patches/` by
  filename convention. Nothing guarantees a published release reconstitutes the
  exact patch set at build time.

CRT v2 therefore **keeps the domain model and replaces the storage
architecture**: a content-addressed store with a sealed, materializable
manifest, built in Rust inside the `cbsd-rs/` workspace so the future axum
service reuses the same core.

### Prior art surveyed

A simpler sibling effort (internal codename "relenor", `release-management/` for
a single customer) was reviewed. It is the **near-dual** of this design — git +
GitHub as the database, metadata riding in-tree on the Ceph fork, integrity via
git's Merkle graph plus a sidecar `sha256`, workflow via GitHub Issues. It
deliberately has **no per-patch visibility** (single customer) and reuses
patches by re-cherry-picking, which confirms our storage choices for the harder
requirements. But its model _above_ the storage layer is more mature than ours,
and §6/§8 below import it wholesale: the risk rubric, the
provenance/justification taxonomy, the cross-release patch lifecycle, the
verification discipline, and GitHub Issues as an interim workflow UI.

## 2. Decisions (locked — brainstorming + prior-art review)

| Axis                            | Decision                                                                                                                                                                                                                           | Consequence                                                                                                                                                                                                          |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Deliverables**                | All four: source patches, patched git ref, built artifacts, signed manifest/SBOM.                                                                                                                                                  | The tool is the canonical source; every representation is _projected_ from it. **Git cannot be the source of truth** — it is one of four outputs.                                                                    |
| **Visibility**                  | Binary, per-patch (`public` / `private`), set by the release manager — but **recorded-only / inert in the MVP** (§7): no effect on SBOM, notes, verify, or materialization. Enforcement is deferred to the future service gateway. | The flag is captured now so the data exists; while the full patched branch ships every patch, redacting derived artifacts would be theater. The gateway enforces it later, with a genuinely reduced public artifact. |
| **Immutability & authenticity** | Digest-sealed (RFC 8785 + `sha256`) **and GPG-signed** — a detached OpenPGP signature over the canonical manifest (and the in-tree record). No/invalid signature ⇒ invalid release. Cosign/sigstore deferred (§11).                | Tamper-evident **and** authentic; the signature ships beside the manifest (S3) and beside the in-tree record, so a detached tree (tarball/ZIP) verifies without git.                                                 |
| **Language / home**             | Rust, in the `cbsd-rs/` workspace.                                                                                                                                                                                                 | Reuse cbsd-rs patterns (sqlx, PASETO, the cbc client model); call cbscore as a subprocess only where build-time materialization needs it.                                                                            |
| **CLI ↔ store**                 | Serverless-first: CLI talks directly to S3 via a shared core.                                                                                                                                                                      | The future axum service reuses that core and adds a DB index + visibility-enforcing read gateway + web UI. S3 is durable truth; DB is later, derived.                                                                |
| **Materialized git history**    | **Linear — no merge commits, ever.** Each patch is a flat `git am` commit carrying a `Crt-Patch` trailer; the `000-RELEASE/` metadata is the final commit. Branch protection on `release-*` / `release/` on the destination repo.  | Per-commit trailers (not merge commits) cross-bind the branch to the manifest; the seal commit sits cleanly at the tip (see §7.3).                                                                                   |
| **Metadata model**              | Adopt relenor's risk rubric, provenance/justification taxonomy, cross-release lifecycle, and `data_structure_change` flag (§6).                                                                                                    | Manifest entries carry structured risk + provenance + lifecycle, not just a hash and order.                                                                                                                          |
| **Interim workflow UI**         | GitHub Issues, on a **configurable** repo (default the downstream fork, likely `clyso/ceph`), **never** the cbs monorepo (§8).                                                                                                     | Triage/approval/cross-release tracking for zero infra, before the web UI exists. Not the store; not visibility enforcement.                                                                                          |
| **Release notes**               | Single projection of the sealed manifest via `minijinja`; config-driven per-channel branding; emitted as `RELEASE-NOTES.md` + into `000-RELEASE/`; authored via `release add` flags + `$EDITOR`.                                   | A presentation layer derived from the manifest, reproducible from a pinned render spec (no separate digest); re-renderable without re-sealing.                                                                       |
| **MVP cut line**                | Store + sealed-manifest core **+ projections (SBOM + release notes) + git materialization** (one `release/<name>` branch + tag in the downstream Ceph repo).                                                                       | Out of MVP: build-system wiring, service/web UI, the GitHub-Issues workflow, **cosign/sigstore** (GPG manifest signing is **in**), and **visibility enforcement** (flag is inert).                                   |

### The unifying primitive — content-addressing

Each patch blob is stored once, keyed by its content hash; releases reference
patches by hash. The hash **is** the dedup ("reused across releases") **and**
the immutability anchor ("immutable once published"). This collapses two of the
six properties into a single mechanism, so the design spends its complexity
budget on the genuinely open axes (backend, visibility, materialization).

## 3. Architecture

A clean three-layer split (matching the cbsd-rs convention of a no-IO shared
crate plus binaries; see `rust-2024`):

- **`crt-core`** — pure domain crate, **no IO, no tokio** (mirrors
  `cbsd-proto`'s role): `Patch`, `PatchSet`, `Manifest`, `Release`,
  `Visibility`, `Risk`, `Provenance`, `Category`; content-hashing;
  canonical-JSON manifest sealing; projection; release-notes rendering
  (`minijinja`, runtime templates — no IO). Fully unit-testable. `thiserror` for
  errors.
- **`crt-store`** — storage trait + an S3 backend (content-addressed blobs +
  JSON manifests/index) and a local-filesystem backend for tests. The trait is
  the seam the future service swaps/extends.
- **`crt` (CLI)** — `clap` binary, cbc-style, composed over the two crates.
  `anyhow` at the top level, `tokio` owned here (not in core).
- **Later, not in this MVP — `crt-server`** (axum) reuses both crates verbatim
  and adds a SQLite index, PASETO auth, the **visibility-enforcing read
  gateway**, and the web UI.

Crates are added at the workspace top level (flat layout, like
`cbsd-proto`/`cbc`), edition 2024, resolver 2, inheriting workspace deps.

```
cbsd-rs/
├── crt-core/     # pure domain (no IO)
├── crt-store/    # storage trait + S3 / local-fs backends
├── crt/          # clap CLI
└── (crt-server/  # future: axum service + DB index + visibility gateway + UI)
```

## 4. Canonical store format (S3)

```
patches/blobs/sha256/<hash>          # immutable patch text (git format-patch output)
patches/meta/sha256/<hash>.json      # visibility-NEUTRAL provenance
                                     #   (author, dates, cherry-picked-from, source PR, patch_id)
releases/<ns>/<channel>/<release>.json   # the sealed manifest + digest
index/releases.json                  # release listing (later: the service's ingest contract)
```

- **Visibility lives in the manifest entry, not the global patch.** The same
  content-addressed patch can be public in release X and private in release Y;
  blobs and meta stay visibility-neutral. A manifest entry carries the hash,
  order, visibility, risk, provenance, justification, and lifecycle fields
  defined in §6.
- **Two hashes, two jobs** (kept separate deliberately — see design §4).
  `blob_hash` = `sha256` of the **raw stored blob** (the exact
  `git format-patch` bytes): a plain artifact content-address giving integrity
  and immutability, and exact reuse when many releases reference the one stored
  blob. `patch_id` (`git patch-id`, offset/whitespace-invariant) = the **logical
  identity**, used for dedup/equivalence at import and for materialized-git
  verification (§7.3) — it survives rebase/`git am` offset, which a diff-byte
  hash does not.

## 5. Sealing, signing & immutability

The sealed release record carries
`digest = sha256(canonical-json(manifest without digest))`, where canonical-json
uses deterministic key ordering (RFC 8785 / sorted keys). Sealing makes the
release **tamper-evident**: any party recomputes the digest to detect
modification.

- **The digest excludes itself.** A hash field inside the bytes being hashed is
  a self-reference paradox; relenor avoids it by keeping `manifest_sha256` in a
  separate `RELEASE.md` record. We do the same: the digest is computed over the
  manifest body and stored alongside it (the release record = manifest body +
  digest envelope), never inside the hashed body.
- **Signed (GPG, in the MVP).** `seal` also produces a **detached OpenPGP
  signature** over the canonical manifest, with Clyso's private key fetched from
  HashiCorp Vault (public key published at a known Clyso-owned location). The
  signature — not just the digest — is the integrity+authenticity anchor: no
  signature or an invalid signature ⇒ invalid release. It ships beside the
  manifest in S3, and beside the in-tree materialization record (§7.3), so a
  **detached tree** (tarball / ZIP download) verifies without git. GPG now;
  cosign/sigstore is a future variant (§11). Crates: `pgp` + `vaultrs` (design
  §6).
- **Not client-enforced write-once.** An operator with bucket credentials can
  overwrite a key. If hard WORM is wanted in the MVP the mechanism is **S3
  Object Lock** on the `releases/` prefix; otherwise the signature already makes
  tampering detectable, and the future service gateway adds access control.
- **The seal predates materialization.** `seal` runs before `materialize`, so
  the sealed manifest keys patches by `blob_hash` only — it cannot contain git
  commit SHAs, which do not exist yet. The git↔`blob_hash` binding is a
  post-seal artifact: the `Crt-Patch` trailers on the materialized commits
  (§7.2), outside the digest, re-derived at verify time (§5.1).

Reuse falls out for free: two releases that share a patch reference the same
hash.

### 5.1 Verification

`crt release verify` runs a layered audit (the schema / cross-file /
git-anchoring layers are adapted from relenor; the signature and re-derivation
layers are new work — relenor has neither):

0. **Signature** — verify the detached OpenPGP signature over the manifest (and,
   for a materialized ref, over the in-tree record) with Clyso's public key. No
   or invalid signature ⇒ fail fast. This is also what lets an external party
   verify a **detached tree** (tarball / ZIP) offline (§7.3).
1. **Schema** — the release record validates against the manifest schema; each
   referenced patch meta validates against the patch schema.
2. **Cross-reference consistency** — the stored digest equals the recomputed
   digest; the set of patch hashes in the manifest equals the set of blobs the
   release references; every referenced blob exists.
3. **Materialized-git anchoring** — when a git ref was materialized, walking the
   linear history matches each commit's `Crt-Patch` trailer to an entry's
   `blob_hash`, and the commit's `git patch-id` to that entry's `patch_id`
   (offset-invariant — survives `git am` offset; it does not fuzz by default,
   and a diff-byte match would survive neither), and re-materializing reproduces
   the same pre-`000-RELEASE` source **tree**. The sealed manifest holds no
   commit SHAs (they post-date the seal); the git↔hash binding is the trailers
   (§7.2), re-derived here.
4. **`000-RELEASE/` faithfulness** — the SBOM/notes committed in `000-RELEASE/`
   are re-derived from the sealed manifest (notes under the pinned render spec)
   and byte-compared; the protected tag covers the tip tree (§7.3).

## 6. Backport & release metadata model (adopted from prior art)

Manifest entries are richer than `{ hash, order, visibility }`. The following
fields are ported from relenor; they make the release auditable and feed both
the SBOM and the release-manager's decisions.

### 6.1 Risk rubric

Four axes, three stored per patch and one derived, summed to a band. Ceph/rgw
calibrated — our exact domain.

| Axis                                 | Values (weight 1→3)                                                        |
| ------------------------------------ | -------------------------------------------------------------------------- |
| `blast`                              | cosmetic / availability / data-loss                                        |
| `conflict`                           | clean / trivial / substantive                                              |
| `coverage`                           | strong / partial / weak                                                    |
| `upstream` (derived from provenance) | merged-stable·merged-main / approved-open / open-in-review·downstream-only |

Sum → band: 4–6 `low`, 7–9 `medium`, 10–12 `high`. **Conventions, not gates**:
the tool _warns_ (e.g. a `high`-band patch without recorded sign-off), humans
decide. The total/band are computed by `crt-core`, not authored.

### 6.2 Provenance & justification taxonomy

- **provenance**: `upstream_pr` (with `upstream_prs[]`, `upstream_commits[]`
  head SHAs, `upstream_pr_state`) or `other` (downstream-only).
- **justification**: `cve` / `customer` / `engineering`, with `refs[]`.
- **Visibility signal.** justification type is a _suggested default_ for the
  release-manager's public/private call: `customer` + internal refs leans
  private; `cve` / `engineering` lean public. A suggestion, never automatic.
- **Public vs internal prose.** Each patch carries a **public** justification
  summary (rendered in release notes) and an optional **internal** justification
  (not rendered into notes). This is a notes-content choice — what is worth
  putting in customer-facing notes — independent of the (MVP-inert) per-patch
  visibility flag (§7).

### 6.3 Cross-release lifecycle

Content-addressing gives identity and dedup; this gives _relevance over time_.

- `patch_status`: `active` / `superseded` / `dropped`.
- `first_shipped_in`: the earliest release that carried this patch.
- **Supersession check**: when a newer upstream release lands, classify each
  carried patch — _absorbed_ (its upstream PR is now reachable upstream → drop),
  _carry-forward_ (still needed → re-score, re-apply), or _review-needed_
  (ambiguous). Automatable from provenance.

### 6.4 Data-structure-change flag

An optional per-patch flag (`struct_v_bump`, `upstream_coordinated`) for patches
that change on-disk / wire struct versions — a downstream data-corruption hazard
if upstream later diverges. Surfaces a "coordination follow-ups" section in
release notes and is walked each release.

### 6.5 Release-notes fields

Release notes (§7.4) are rendered from the sealed manifest, so the notes content
must be authored into the manifest at compose time:

- **`category`** per patch (`security` / `feature` / `fix` / `integration`) —
  the grouping axis for the notes sections.
- **Per-patch prose**: `public_summary` (the public justification of §6.2), plus
  optional `behavior_change` and `upgrade_notes`. Notes render the public prose;
  internal justification is not rendered.
- **Release-level**: `known_issues` and a release-wide `upgrade_notes`.

These are authored at `release add` time (flags + `$EDITOR`, see §9) — the same
human step relenor budgets for; the GitHub Issue Form becomes the richer
authoring surface post-MVP (§8).

## 7. Materialization (visibility recorded, inert in MVP)

### 7.1 One tier in the MVP

The per-patch `visibility` flag is **recorded but inert** in the MVP: it does
not filter or redact the SBOM, the release notes, verification, or the
materialized git ref. A `private` patch is treated exactly like any other. The
rationale is that the **full patched git branch is itself a deliverable** and
contains every patch, so redacting the derived artifacts while shipping the
whole tree would be theater. So there is **one** projection (all patches, all
rendered fields), one SBOM, one set of notes, one git ref.

Capturing the flag now means the data exists for when it becomes meaningful:
**visibility enforcement is deferred to the future service gateway** (§8), which
will mediate reads by entitlement and serve a genuinely reduced public artifact.
So property 4 ("selectively visible") is, in the MVP, **modeled but not
enforced** — honestly, not even segregated yet.

Binaries are built from the full ordered set (build-wiring, deferred — §11); the
MVP yields source patches, SBOM, notes, and the git ref, not built artifacts.
(LGPL note: binary delivery can carry a source-availability obligation — a
design edge, out of scope here.)

### 7.2 Git materialization (in MVP)

Driven by a sealed manifest, produce **one** `release/<release>` branch + tag in
the downstream Ceph repo, applying every patch.

**Linear history, no merges.** Unlike relenor (whose merge commits are the audit
anchor), our branch is flat: each patch is one `git am` commit, in manifest
order, carrying a `Crt-Patch: sha256:<hash>` trailer (plus a `Crt-Visibility`
trailer recording the inert flag). The trailer cross-binds each commit to its
manifest entry — the same audit guarantee relenor gets from merge trailers, on a
linear branch. A patch that fails to apply aborts the run and **fails loud**;
never produce a silently-wrong branch. Branch/tag protection on `release-*` /
`release/` on the destination repo keeps the published refs from moving.

Patches are applied by shelling out to `git am` / `git format-patch` / `git tag`
/ `git push` (the subprocess pattern already used by `cbsd-worker`), against a
clean checkout of the configured base ref. `gix`/`git2` is a possible later
optimization.

### 7.3 In-tree release metadata — the `000-RELEASE/` seal commit

The **final** commit on the materialized branch adds a top-level `000-RELEASE/`
directory — a **signed, public-safe verification bundle** that makes the git ref
a self-describing, verifiable deliverable **even when detached from git**
(tarred, or "Download ZIP" from the GitHub UI). It is **not** the sealed S3
manifest: the full manifest stays in S3, and the in-tree bundle is a public-safe
projection that omits `visibility` and `justification.internal` (those never
leave S3). Because our history is linear with no later merges, this commit sits
at the tip and is never perturbed.

Contents:

- `record.json` — the **materialization record** (public-safe BOM): the sealed
  manifest's digest as a back-reference (`s3_manifest_digest`), the base ref, a
  `source_tree_digest` over the materialized files (excluding `000-RELEASE/`), a
  digest of **every other `000-RELEASE/` file** (sbom, notes, provenance, README
  — so nothing in the bundle is unsigned), and per-patch
  `{ blob_hash, patch_id, git_commit }`. No `visibility`, no internal prose.
- `record.json.asc` — the **detached OpenPGP signature** over `record.json`
  (Clyso's key, §5): the portable root of trust.
- `sbom.cdx.json` — the CycloneDX SBOM (§7.4 / design §7.1).
- `RELEASE-NOTES.md` — the rendered notes (§7.4).
- `provenance.json` — public patch list (provenance / risk / public
  justification).
- `README.md` — what this is and how to verify.

Resolved sub-decisions:

- **Visible, namespaced directory** (`000-RELEASE/`), not hidden — relenor runs
  this in production; CMake ignores an unreferenced top-level dir.
- **It is NOT `export-ignore`d.** The bundle must **travel with** the tree so a
  tarball / ZIP recipient can verify offline; it is inert for builds, and the
  deferred build path excludes it from the Ceph build tarball. (This reverses an
  earlier `export-ignore` note, which conflicted with detached verification.)
- **Signature, not just digest.** A git tag is lost the moment the tree leaves
  git; the in-tree detached signature is what makes a detached tree verifiable.

**Integrity — three independent legs.** (1) **Portable (primary):** from a
tarball/ZIP — no git, no store — verify `record.json.asc` with Clyso's public
key ⇒ record authentic, then recompute `source_tree_digest` and **every**
bundled-file digest over the extracted files ⇒ contents authentic. (2)
**Git-native:** the protected annotated tag commits to the tip tree via git's
Merkle hashing (a future signed tag adds cryptographic authenticity for clones).
(3) **Internal faithfulness (Clyso, with S3):** re-derive the SBOM/notes from
the sealed manifest (SBOM a pure function of it; notes under the pinned render
spec) and byte-compare, and confirm the in-tree record is a faithful projection
of the sealed manifest (its `s3_manifest_digest` matches). Together the git
output inherits the manifest's tamper-evidence rather than reintroducing soft
immutability.

### 7.4 Release notes

Release notes are a projection of the sealed manifest, a sibling of the SBOM —
same machinery (read manifest → render) plus a template:

- **Renderer**: `minijinja` in `crt-core` (runtime, no IO), with a **pinned
  version** and the **template recorded by digest in the sealed manifest** (its
  bytes stored content-addressed so `verify` can fetch the exact template — see
  design §5) so the notes are a deterministic projection `verify` can reproduce
  (§7.3). The default template **ports with adaptation** from the Python `crt` /
  relenor `RELEASE-NOTES.md.j2` (grouped by category, per-patch sections, a
  cumulative active-patches table, known issues, a "coordination follow-ups"
  section fed by `data_structure_change`) — _not_ literally verbatim, since
  relenor uses a custom `groupby` global + `selectattr`/`sort(attribute=)` that
  map differently in minijinja (design §7.2).
- **Branding (config-driven)**: a per-channel `branding` block (display name,
  blurb, footer, section labels) the default template reads — cheap given
  minijinja. The default template is the fallback; per-channel _custom template
  files_ are deferred (§11).
- **Content**: notes render each patch's public summary (not the internal
  justification — a content choice, §6.2). The visibility flag does not affect
  them (§7.1).
- **Emission**: a standalone `RELEASE-NOTES.md` artifact **and** a copy into
  `000-RELEASE/` (§7.3). Notes are a presentation layer derived from the sealed
  manifest — reproducible from the pinned render spec, **no separate digest**;
  `crt release notes` re-renders without re-sealing. Rendering with a
  _different_ template is a **preview**; the canonical notes match the sealed
  render spec.

## 8. Interim workflow UI — GitHub Issues (post-MVP, designed-for)

Before the web UI exists, GitHub Issues provides triage/approval/cross-release
tracking for zero infrastructure (relenor's approach).

- **Configurable repo.** The issues repo is a config field, defaulting to the
  downstream fork (likely `clyso/ceph`). It is **never** the cbs monorepo
  (`cbs.git`) — that holds tooling, not product release workflow. Putting issues
  on the fork itself is deliberately _simpler_ than relenor's separate
  tooling/issues repo: same-repo writes, no cross-repo PAT (relenor needed an
  `ISSUES_REPO_TOKEN` precisely because its issues lived elsewhere).
- **What it tracks**: backport-candidate triage (Issue Forms), approval status
  (labels), per-effort grouping (milestones), and a cumulative "shipped in"
  footer updated on release.
- **Boundaries**: it is a _workflow/tracking convenience_, **not** the store (S3
  stays canonical) and **not** visibility enforcement (issue visibility is the
  repo's, not per-patch). Authored risk/provenance still lands in the manifest,
  not the issue.

## 9. Lifecycle / CLI surface

```
crt patch import        # GitHub PR or git range → content-address blob + neutral meta
crt release new         # start a draft release record (base repo/ref, namespace/channel)
crt release add         # add patch(es); category/visibility/risk/justification (flags)
                        #   + prose (public summary/behavior change/upgrade notes) via $EDITOR
crt release seal        # draft → sealed: digest + GPG-sign (key from Vault), write record
crt release materialize # project: SBOM + notes + the linear git ref/tag (000-RELEASE/)
crt release notes       # (re-)render release notes from the sealed manifest
crt release list | info | verify   # query + layered verify (§5.1)
crt release supersede   # classify carried patches vs a new upstream release (§6.3)
```

## 10. Implementation roadmap (capability milestones)

Per `git-commits`, work is sequenced by **capability delivered**, not by layer —
a `crt-core` crate with no caller is dead code, not a shippable commit. Each
milestone is a user-visible capability; detailed commit breakdowns (sized
~400–800 LOC, approved before coding) live in the corresponding `crt/plans/`
document.

| #      | Capability (what an operator can do after it)                                                                                           | Spans                                                                                             |
| ------ | --------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| **M1** | Import a patch (GitHub PR or git range); it lands content-addressed in the store with neutral provenance.                               | `crt-core` (types, addressing) + `crt-store` (S3/local) + `crt patch import`                      |
| **M2** | Compose a release with per-patch category, visibility, risk, provenance/justification, prose, and lifecycle; seal it; verify (layered). | `crt-core` (manifest, risk rubric, sealing, verify) + `crt release new/add/seal/list/info/verify` |
| **M3** | Produce the **SBOM + release notes** (config-driven branding) from a sealed release.                                                    | `crt-core` (projection, `minijinja` render) + `crt release materialize` + `crt release notes`     |
| **M4** | Materialize the single **linear** `release/<name>` branch + tag with `Crt-Patch` trailers and the **signed** `000-RELEASE/` bundle.     | `crt release materialize` (git) + subprocess git driver                                           |

Preceding all four: the `crt/design/001-…` design document and a decision record
for the S3-canonical / content-addressing choice. Post-MVP, in order: the
supersession lifecycle (`crt release supersede`), the GitHub-Issues workflow
(§8), build-system wiring, **cosign/sigstore signing**, and the service + web
UI.

## 11. Out of scope for the MVP (deferred, but designed-for)

- **GitHub-Issues workflow** (§8) — interim UI; planned post-MVP.
- **Build-system wiring** — `cbsbuild` consuming a sealed manifest to
  reconstitute the exact patch set at build time, using relenor's **no-rebuild**
  discipline: re-tag tested artifacts, re-verify digests against the manifest
  ("the bytes tested are the bytes shipped"). Closes the producer/consumer gap.
- **Per-channel custom release-notes templates** — the MVP ships a default
  template with config-driven branding (§7.4); channel-specific _template file_
  overrides (and the polished brand/legal copy itself) come later.
- **Service + web UI** — `crt-server` (axum) + SQLite index +
  visibility-enforcing read gateway + web UI.
- **cosign/sigstore signing** — a future variant of the in-MVP GPG signing (§5);
  see design §6.2. (GPG manifest signing itself is **in** the MVP.)
- **Vault-sourced S3 credentials** — MVP uses the standard AWS credential chain.
- **Migration** from the Python `crt` store.

## 12. Reuse / prior art

- **relenor** (`release-management/`, reviewed) — source of the risk rubric
  (§6.1), provenance/justification taxonomy (§6.2), cross-release lifecycle
  (§6.3), `data_structure_change` flag (§6.4), the layered verify (§5.1),
  digest-excludes-itself (§5), per-commit/merge trailers (§7.2), no-rebuild
  build discipline (§11), and GitHub-Issues workflow (§8).
- **Domain model** to port:
  `crt/src/crt/crtlib/models/{patch,patchset,manifest}.py`.
- **GitHub PR → patches** logic: `crt/src/crt/crtlib/github.py`.
- **Release-notes templates** to port (Jinja → minijinja): relenor's
  `templates/RELEASE-NOTES.md.j2` and the Python `crt` release-notes templates.
- **Namespace/channel config** concept: the in-flight redesign's
  `crt.config.yaml` + `crtlib/paths.py` — port the config-driven identity, drop
  the git-branch coupling. Resolution is **prefix-based**; no Ceph codename
  table is needed for the MVP.
- **S3 layout conventions** (reference, reimplemented in Rust):
  `cbscore/src/cbscore/utils/s3.py`, `cbscore/src/cbscore/releases/s3.py`.
- **Version parsing** (reference): `cbscore/src/cbscore/versions/utils.py`.
- **Rust patterns**: `cbsd-rs/cbc` (clap layout, config, output),
  `cbsd-rs/cbsd-worker` (subprocess management), `cbsd-rs/CLAUDE.md`
  (fmt→clippy→check, error handling, commit conventions).

## 13. Open questions

Enumerated here pre-design and now **settled in design §1**: CLI/crate name
(`crt`), license (**GPL-3.0-or-later**), commit prefix (`crt:`), S3 credentials
(AWS chain; Vault is used for the signing key, not S3), hard WORM
(tamper-evidence + optional S3 Object Lock), risk `component` (a configurable
list, not an enum), git ops (subprocess), SBOM format (CycloneDX), and the
signing crates (`pgp` / `vaultrs`).

Two earlier questions are **mooted** by the single-tier MVP (visibility inert,
§7): the "public-subset apply policy" and "public SBOM redaction" — there is no
public-only branch or redaction left to decide.

Remaining design-level open items live in **design §14** (key management,
`source_tree_digest` algorithm, `patch_id` edge cases, crate currency).

## 14. Verification (MVP acceptance)

- **`crt-core`** unit tests: content-hashing determinism; canonical-JSON +
  digest stability (seal twice → identical digest); digest excludes itself; risk
  total/band computation; projection is identity (the inert `visibility` flag
  changes nothing); reuse (shared hash across two manifests); release-notes
  rendering (category grouping; renders public summary not internal prose;
  deterministic re-render under a fixed render spec).
- **`crt-store`** tests: round-trip blob/meta/release through the local-fs
  backend; digest tamper-evidence (mutate a sealed release → `verify` fails);
  integration against real/minio S3 where available.
- **End-to-end**: `patch import` (a real Ceph PR) → `release new/add` (with risk
  - justification; some entries flagged `private` to prove the flag is inert) →
    `seal` → `verify` (layered) → `materialize` → assert one `release/<name>`
    branch containing **all** patches, linear history with matching `Crt-Patch`
    trailers and a `000-RELEASE/` tip commit, the tag exists in a scratch
    downstream repo, and `verify` re-derives a byte-identical `sbom.cdx.json` +
    `RELEASE-NOTES.md`.
- Per `cbsd-rs/CLAUDE.md`: `cargo fmt --all`, `cargo clippy --workspace`,
  `cargo check --workspace` (and `cargo sqlx prepare` only once DB queries
  appear — none in the MVP).
