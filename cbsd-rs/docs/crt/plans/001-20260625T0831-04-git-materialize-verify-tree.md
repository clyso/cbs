# CRT v2 — Plan M4: git materialization, signed bundle, `verify --tree`

> **Status:** Plan (**approved** 2026-06-25; revised per the v1 plan review
> [`../reviews/001-20260625T0846-plan-v2-mvp-v1.md`](../reviews/001-20260625T0846-plan-v2-mvp-v1.md)
> and the v2 review
> [`../reviews/001-20260625T0917-plan-v2-mvp-v2.md`](../reviews/001-20260625T0917-plan-v2-mvp-v2.md)
> — verifier-first ordering, blockers F1/F2 resolved, F3–F9/F11 folded; v2's
> N2/F11 folded and its N1 overridden per that review's maintainer disposition).
> Implements **M4** of the design
> [`../design/001-20260620T1318-v2-mvp.md`](../design/001-20260620T1318-v2-mvp.md)
> (§8 git materialization + signed `000-RELEASE/`, §11 verify legs incl. the
> ref-conditional checks in legs 0–4, `crt verify --tree`). Part 04 of the
> multi-part plan sharing seq `001` (M1 = 01, M2 = 02, M3 = 03). Commit
> boundaries follow the `git-commits` skill (capability per commit, ~400–800 LOC
> soft, lockfiles excluded). Breakdown **approved**; implementation proceeds at
> 4.1.

## Scope

After M3 an operator can author, seal, verify (legs 0–2), and emit the two
deterministic artifacts of a sealed release. M4 is the **last MVP milestone**:
it turns a sealed release into a **publishable, independently verifiable git
artifact** and closes the verification model.

- **Git materialization (§8):** one linear `release/<name>` branch — `git am`
  each entry's blob in `order`, each commit amended with a
  `Crt-Patch: sha256:<blob_hash>` trailer (no `Crt-Visibility`) — plus the final
  **`000-RELEASE/`** commit (the portable, public-safe verification bundle:
  `record.json` and its detached `record.json.asc`, `sbom.cdx.json`,
  `RELEASE-NOTES.md`, `provenance.json`, `README.md`), and an **annotated tag**
  carrying the manifest digest.
- **`crt verify --tree <dir>` (§10):** offline / detached-tree verification — no
  store, no git — the **primary** trust path for a tarball/ZIP/clone recipient:
  verify `record.json.asc` with the public key, recompute `source_tree_digest`
  and every `bundle_digests` entry, compare.
- **Store-backed verify, ref-conditional (§11):** when a materialized ref
  exists, `crt release verify` additionally runs the bundle-signature check (leg
  0), the in-tree-record schema validation (**leg 1**) and cross-reference (leg
  2), git anchoring (leg 3), and artifact faithfulness (leg 4) — instead of
  reporting them `skipped`.

This completes design §11 and the §10 CLI surface; nothing in the MVP design
remains unbuilt after M4.

**Verifier-first ordering (v1 review F10).** The load-bearing risk is
`source_tree_digest` cross-environment determinism (F1). So the `crt-core`
hashing pieces and the offline `verify --tree` land **before** the bundle
producer that depends on them — the digest algorithm is finalized and
fixture-proven against the real `destination_repo` first, not discovered late
inside the producer.

**Out of M4 (post-MVP / deferred):** GPG-**signed** git tags (the annotated tag
and the detached `record.json.asc` are the MVP anchors; a signed tag covering
the git graph is §8 future work); cosign/sigstore (§6.2); key rotation /
revocation / fingerprint-pinning (§6.1/§14); `visibility` enforcement / a
genuinely reduced public artifact (§7 — inert in the MVP); the GitHub-Issues
authoring workflow (concept §8); `index/releases.json` as a maintained cache
(§5); the Ceph build-tarball path that excludes `000-RELEASE/` (concept §11).
The v5-review deferral of the **CycloneDX validator** and the issue
`references`/`iri-reference` shape (M3 backlog, F6b) stays deferred unless a
validator is wired in.

## Decisions (ratified with the maintainer)

1. **Destination repo (v1 review F4).** Reuse the **existing design §9 config**:
   the top-level `destination_repo` (e.g. `clyso/ceph`) is where the
   `release/<name>` branch + tag are built, and per-channel `upstream.repo` is
   the patch source. Add the `destination_repo` field to the `Config` struct
   (the design names it; it is not yet in code) plus a `--repo <path>` CLI
   override. M4 assumes no particular remote layout beyond these.
2. **Push.** M4 builds the branch + tag + bundle **locally**; `git push` is
   **opt-in** (a `--push` flag, exercised only by an `#[ignore]`d env-gated
   test) — mirroring the M2/M3 Vault/S3 edge pattern so `cargo test` stays
   network- and credential-free.
3. **`source_tree_digest` — byte domain (§14; v1 review F1; finalized +
   fixture-proven in 4.2).** The verifier runs with **no git**, so the digest
   must be computable from on-disk files alone, and the producer's materialized
   worktree must yield byte-identical files to what a recipient extracts.
   **Rules:** `sha256` over a canonical serialization of sorted entries — one
   per non-excluded regular file — each
   `(relative-slash-path, sha256(on-disk content bytes))`; **exclude**
   `000-RELEASE/` and `.git/`. To make the producer's worktree bytes equal the
   stored blob bytes (hence any faithful extraction), materialize checks out
   with git content filters **disabled** (`core.autocrlf=false`, no
   clean/smudge), and the bundle's `.gitattributes` gets `000-RELEASE/* -text`
   so the signed files are never EOL-mangled downstream. **Open sub-decisions
   settled in 4.2 against the real `destination_repo`:** (a) audit its
   `.gitattributes` for any transforming rule (`text=auto`, `eol`, smudge) that
   desyncs a checkout from the blob bytes; (b) the executable-bit and symlink
   treatment — a ZIP recipient (a §8 primary path) drops the exec bit and may
   collapse symlinks, so **lean toward hashing content only (ignore mode) and
   recording symlinks by target**, then prove digest-equality between the
   producer worktree and a `tar`-extracted tree in a fixture that **includes an
   attributes file, an executable file, and a symlink**. The canonical
   distribution is a plain archive of the worktree (minus `.git/`), **not**
   `git archive` (which re-applies attributes).
4. **`materialize --out` fate.** M3's `--out` writes the two loose artifacts;
   their canonical home in M4 is `000-RELEASE/`. Keep `--out` as an optional
   _extra_ emit (handy for inspecting artifacts without a git checkout); the
   bundle in the git ref is authoritative.
5. **`MaterializationRecord` schema version (v1 review F2; design addition).**
   Design §3's struct has no `schema_version`, but §11 **leg 1** requires it to
   deserialize-and-validate. Add a `schema_version` field plus a
   `MATERIALIZATION_RECORD_VERSION` constant in `crt-core` (mirroring
   `SCHEMA_VERSION`/`Manifest`). Recorded here as an intentional deviation from
   the design-as-written.

## Reflected design invariants & decisions

- **`crt-core` stays pure (no IO / no tokio).** The new pure pieces —
  `MaterializationRecord`/`MaterializedPatch` (+ `schema_version`), a
  `PublicProvenance` projection (F3), and `source_tree_digest`'s hash
  **combine** over an in-memory `BTreeMap<path, content_hash>` — live in
  `crt-core`. The **directory walk** (IO), subprocess `git`, Vault key fetch,
  and the public-key fetch are all `crt`-only edges. The determinism risk
  (decision 3) lives in the **walker**, so the walker is fixture-tested in `crt`
  (4.2).
- **Detached-sign reuses `sign_manifest` as-is (F7).** `sign_manifest` already
  signs arbitrary `&[u8]`, so `record.json.asc` is just
  `sign_manifest(rng, record_json_bytes, key, pass)` — no generalization (an
  alias/rename for readability is optional, not required).
- **The bundle is signed-by-construction (§8).** `bundle_digests` carries a
  `sha256` of **every other** `000-RELEASE/` file, and `record.json.asc` signs
  `record.json` — so the single detached signature transitively covers the whole
  bundle. The `000-RELEASE/` commit is created **once** with all its files,
  including `record.json.asc`; signing therefore happens **before** that commit.
- **Public-safe by construction (§3, F3).** `MaterializationRecord` has **no**
  `visibility`/`internal`; `provenance.json` is a typed `PublicProvenance`
  projection (not a raw `PatchMeta` dump), so the bundle cannot leak downstream
  classification by omission. The branch carries **no** `Crt-Visibility`
  trailer.
- **Two hashes, two jobs (§4); patch-id reuse is partial (F8).** Leg 3 reuses
  the `git patch-id --stable` invocation (`git_with_stdin`, `import.rs:239`) but
  the **input is new**: it derives the diff bytes from each materialized
  **commit** (`git show`/`format-patch -1`) rather than a stored blob, then
  pipes that to `patch-id`, and compares to the entry's `patch_id`
  (offset-invariant). `blob_hash` is the byte-exact address in the `Crt-Patch`
  trailer.
- **Re-derivation determinism is tree-level, not commit-level (F6).** `git am`
  embeds author/committer dates, so commit SHAs are **not** reproducible across
  runs. Leg 3 therefore anchors via the `Crt-Patch` trailer + recomputed
  `patch_id` per commit, and compares the **pre-`000-RELEASE` source tree** via
  `source_tree_digest` against the in-tree `record.json` value — it does **not**
  compare commit SHAs and does **not** require a full re-`git am`.
- **External consumers run only `verify --tree`** (signature + digests), never
  leg 4 (§11) — leg 4 is Clyso's internal faithfulness audit and depends on the
  M3 re-derivation engines.
- **Fail loud (§8).** A patch that fails to `git am` aborts the run
  (`git am --abort`) and errors; a half-materialized ref is never left behind.

## Commits (4; verifier-first: branch → digest+verifier → bundle → audit)

> Present for approval before coding. Each delivers an observable, independently
> testable capability and exercises what it adds — no dead code.

### 4.1 — `crt: materialize a sealed release into a linear git branch`

**After this:** `crt release materialize <name>` (besides the M3 artifacts)
builds the linear `release/<name>` branch in a clean checkout of
`destination_repo` at `base_ref` — `git am` each entry's blob in `order`, amend
a `Crt-Patch: sha256:<blob_hash>` trailer; fail-loud + `git am --abort` on any
apply failure. **No `000-RELEASE/` bundle and no tag yet.**

- `crt`: `destination_repo` resolution (decision 1); clean checkout/worktree at
  `base_ref` with content filters disabled (decision 3); the `git am` +
  trailer-amend loop (new helpers in `git.rs`).
- **No carried-forward state (F5):** 4.1 does **not** persist commit SHAs for a
  later commit — the bundle (4.3) rebuilds the patch BOM by walking the branch's
  `Crt-Patch` trailers. 4.1's own tests read the SHAs only to assert structure.
- **Tests:** materialize a small synthetic base repo + sealed release → assert
  linear history, one commit per entry in `order`, each with the right
  `Crt-Patch` trailer and **no** `Crt-Visibility`; an apply-conflict fixture
  aborts and errors (no dangling branch).
- **Smell test:** one capability (produce the patched branch); reverts cleanly.
  **~500–650 LOC.**

### 4.2 — `crt: hash the source tree and verify a tree offline`

**After this:** `crt verify --tree <dir>` verifies an extracted tree with **no
store and no git** (§10) — the primary path for external recipients — and the
`source_tree_digest` algorithm is finalized and fixture-proven. _(Verifier-first
per F10: this lands before the producer 4.3 that emits what it verifies; until
4.3 it is tested against a hand-built, test-key-signed `000-RELEASE/` fixture.)_

- `crt-core`: `MaterializationRecord`/`MaterializedPatch` (+ `schema_version` +
  `MATERIALIZATION_RECORD_VERSION`, decision 5; a `created` timestamp sourced at
  materialize time, F11), `PublicProvenance` (F3), and the pure
  `source_tree_digest` combine.
- `crt`: the directory walker feeding `source_tree_digest` (excludes
  `.git/`/`000-RELEASE/`, decision 3); `crt verify --tree` — read
  `record.json` + `record.json.asc`, fetch the public key (reuse leg 0's
  https-or-local loader), verify the signature, recompute `source_tree_digest`
  and **every** `bundle_digests` entry over the extracted files, compare;
  distinct exit codes (`VerifyVerdict`). Also assert `bundle_digests` is
  **exhaustive** — it lists every `000-RELEASE/` file except
  `record.json`/`record.json.asc`, with no missing or extra entry — so any file
  4.3 later adds to the bundle is forced under the signature (v2 review N2).
- **Finalize decision 3 here:** audit the real `destination_repo`
  `.gitattributes`; settle exec-bit/symlink handling; the determinism fixture
  must prove **worktree ≡ `tar`-extracted tree** with an attributes file, an
  executable file, and a symlink present.
- **Tests:** the determinism fixture above; `verify --tree` on the fixture
  bundle passes offline and **fails** on a mutated source file, a mutated
  `sbom.cdx.json`, a stripped/wrong-key signature, and a `000-RELEASE/` file
  absent from `bundle_digests` (exhaustiveness, N2) — each with the expected
  exit code.
- **Smell test:** one capability (offline tree verification + the pinned
  digest); reverts cleanly. **~600–700 LOC** (the gating-risk commit).

### 4.3 — `crt: append the signed 000-RELEASE/ bundle + tag`

**After this:** materialize appends the `000-RELEASE/` commit and an annotated
tag carrying the manifest digest; `--push` (decision 2) optionally publishes.
End-to-end: `materialize` then `verify --tree` (4.2) passes on real output.

- `crt`: walk the branch to build the patch BOM (`Crt-Patch` trailer + commit
  SHA per entry); assemble `record.json` (back-ref `s3_manifest_digest`,
  `base_ref`, `RenderSpec`, `created`, `source_tree_digest`, the BOM,
  `bundle_digests`); write `sbom.cdx.json`/`RELEASE-NOTES.md` (reuse M3),
  `provenance.json` (the `PublicProvenance` projection), `README.md`, and
  `.gitattributes` (`000-RELEASE/* -text`); compute `bundle_digests` over every
  other file; **sign `record.json` via the Vault key (edge), reusing
  `sign_manifest`**; create the `000-RELEASE/` commit with all files incl.
  `record.json.asc`; annotated tag; opt-in `--push`.
- **Tests:** `bundle_digests` covers every non-record file; `record.json.asc`
  verifies against the test key; `verify --tree` (4.2) passes on the freshly
  materialized tree; the record + `provenance.json` are public-safe (no
  `visibility`/`internal`/`Crt-Visibility`); tag present at the bundle tip.
  Vault mocked; `--push` `#[ignore]`d.
- **Smell test:** one capability (emit the signed verification bundle); reverts
  cleanly. **~500–650 LOC.**

### 4.4 — `crt release verify`: activate the ref-conditional legs (0–4)

**After this:** when a materialized ref exists, `crt release verify <name>` runs
the previously-`skipped` checks instead of reporting them skipped.

- **Leg 0 (extend):** also verify `000-RELEASE/record.json.asc` over
  `record.json` with the public key.
- **Leg 1 (extend, F2):** the in-tree `MaterializationRecord` deserializes and
  validates, including its `schema_version` against
  `MATERIALIZATION_RECORD_VERSION`.
- **Leg 2 (extend):** the in-tree record's `s3_manifest_digest` == the sealed
  digest, and its patch BOM is a faithful projection of the sealed entries.
- **Leg 3 (new, F6/F8):** walk the linear history — each commit's `Crt-Patch`
  trailer names its entry's `blob_hash`, and the `patch_id` recomputed **from
  the commit's diff** equals the entry's `patch_id` (offset-invariant, per §11).
  Computing from the **commit** (not the stored blob) is deliberate: it verifies
  what `git am` actually landed, and `patch-id --stable` is offset-invariant so
  the match holds even when the patch applied at an offset — feeding the stored
  blob would only re-derive the import-time `patch_id` and prove nothing (v2
  review N1, overridden; see that review's maintainer disposition). Then
  recompute `source_tree_digest` over the checked-out pre-`000-RELEASE` tree and
  compare to the in-tree value.
- **Leg 4 (new):** re-derive `sbom.cdx.json` (pure) and re-render
  `RELEASE-NOTES.md` (sealed `RenderSpec`, version-gated) and **byte-compare**
  to the committed copies (the M3 engines). **Folds in v5-review F2:** pin
  `serde_json` **exact** here, with leg 4's byte-compare.
- **Reporting (F9):** add a `Failed` arm to `LegState` so the per-leg report
  names the failing leg; the run still resolves to the existing
  `VerifyVerdict::{SignatureFailed, VerifyFailed}` for exit codes.
- **Tests:** leg 3 passes including an **offset fixture** — base has an
  unrelated earlier edit, so the patch applies at an offset and the materialized
  commit's diff bytes differ from a clean apply, yet its `patch_id --stable`
  still anchors to the entry (the stored blob is unchanged); leg 4 byte-matches;
  legs 0–4 each **fail** on the corresponding tamper; legs still report
  `skipped` when no ref exists.
- **Smell test:** one capability (close the verification model); reverts
  cleanly. **~500–650 LOC.**

## Carry-forward invariants (do not regress)

- `crt-core` purity (no IO/tokio); no `openssl`/`native-tls` in the tree.
- Canonical manifest bytes are a signed contract (the 2.1 byte golden guards
  it); the M3 **SBOM** byte golden likewise guards leg 4's input.
- `SCHEMA_VERSION` unchanged (no manifest schema change in M4);
  `MATERIALIZATION_RECORD_VERSION` is **new** and independent.
- Notes render the branding **snapshot**, version-gated minijinja, template by
  digest — never live config (leg 4 depends on this).
- Seal order / write-once (M2) unchanged; materialize is **read-only** w.r.t.
  the sealed `ReleaseRecord` (it consumes, never rewrites it).

## Verification (end-to-end)

Gate (from `cbsd-rs/`): `cargo fmt --all`,
`cargo clippy --workspace --all-targets`, `cargo test --workspace`. Plus the §12
integration arc against a synthetic base repo + `object_store::InMemory` (no
Vault/S3/network): `import → new → add → seal → materialize` builds the
branch/bundle/tag; `verify --tree` on the **extracted tree** passes and fails
under tamper; store-backed `verify` passes legs 0–4 including the **offset**
fixture for leg 3. The `source_tree_digest` worktree-≡-tarball fixture
(attributes + exec + symlink) gates decision 3. Live push / Vault /
public-key-URL paths stay `#[ignore]`d + env-gated.

## Review cadence (per STATUS.md / project workflow)

Implement 4.1–4.4 → adversarial review of the group
(`pr-review-toolkit:code-reviewer` via `adversarial-review`, with
`confidence-scoring` + `seq-docs-convention`) → address findings via
`git commit --fixup=<introducing-commit>` → review doc (`…-impl-v2-mvp-v6.md`)
as a standalone commit → **user runs the autosquash**. Commits: `crt:` prefix
(docs `crt/docs:`), DCO `-s`, never GPG-sign autonomously (`--no-gpg-sign`), one
`Co-authored-by`, separate `git add`/`commit`.

## Progress

| Commit | Subject                                               | Status |
| ------ | ----------------------------------------------------- | ------ |
| 4.1    | materialize a sealed release into a linear git branch | ☐ todo |
| 4.2    | hash the source tree and `verify --tree` offline      | ☐ todo |
| 4.3    | append the signed `000-RELEASE/` bundle + tag         | ☐ todo |
| 4.4    | activate the ref-conditional verify legs (0–4)        | ☐ todo |
