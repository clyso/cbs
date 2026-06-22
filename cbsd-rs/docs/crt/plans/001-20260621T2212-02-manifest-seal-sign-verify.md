# CRT v2 — Plan M2: sealed, signed release manifests + verify

> **Status:** Plan (approved; M2 in progress). Implements **M2** of the design
> [`../design/001-20260620T1318-v2-mvp.md`](../design/001-20260620T1318-v2-mvp.md)
> (concept [`../000-concept.md`](../000-concept.md)). Part 02 of the multi-part
> plan sharing seq `001` (M1 is part 01). Commit boundaries follow the
> `git-commits` skill (capability per commit, ~400–800 LOC soft, lockfiles
> excluded). **No code lands before this breakdown is approved.**

## Scope

After M1 an operator can import patches into a content-addressed store. M2
delivers the **release** lifecycle: compose a draft from imported patches,
**seal** it (RFC 8785 canonical JSON → `sha256` digest → detached OpenPGP
signature with a Vault-fetched key), persist the sealed `ReleaseRecord` in the
store, and **verify** a sealed release (signature + schema + cross-reference).

Design coverage (per §13): §3 manifest types, §6 sealing + signing (Vault +
rPGP), the §7 projection **seam** (identity; `visibility` recorded but inert),
§11 verify **legs 0–2**, and the `release new/add/seal/list/info/verify` CLI.

**Out of M2** (later milestones): deterministic SBOM + release notes rendering
(§7.1/§7.2, M3); `release materialize` + git ref/tag + signed `000-RELEASE/`
bundle + `crt verify --tree` (§8, M4); verify **legs 3–4** (git anchoring +
artifact faithfulness — need a materialized ref, M3/M4).

**Reflected design invariants.** `crt-core` stays pure (no IO/tokio): it signs
and verifies over **in-memory key bytes**, so the whole seal/verify pipeline is
unit-testable without Vault or the network. The `vaultrs` private-key fetch and
the public-key fetch are **thin edge shims** in the `crt` binary that hand bytes
to `crt-core` — mirroring M1's "octocrab supplies metadata only" discipline.

**Drafts live in the shared store, not on local disk** (revises the design's
"drafts live locally"). A draft on one operator's laptop strands the work: a
second operator cannot continue it, and a release cannot be completed if the
author is unavailable. Drafts are therefore **mutable** store objects under a
`drafts/` prefix — distinct from the **write-once** sealed `releases/` — so any
operator with store access can `new`/`add`/`seal`. MVP concurrency is **serial
handoff (last-writer-wins)**: concurrent edits to the same draft can lose an
update; safe collaboration is one editor at a time. True concurrent editing
(optimistic-concurrency conditional puts, or the post-MVP GitHub-Issues
workflow, concept §8) is deferred.

## Commits

### 2.1 — `crt: model release manifests with a canonical-JSON digest`

**After this:** `crt-core` can build a `Manifest`, serialize it to RFC 8785
canonical JSON, compute `digest = sha256(canonical_json(manifest))`, and score
each entry's risk — deterministically.

- `crt-core`: `Manifest`, `ManifestEntry`, `ReleaseHeader`, `ReleaseRecord`
  (envelope = manifest + `digest` + `signature`), `Draft` (the mutable, pre-seal
  manifest body — header + entries + known-issues, no
  `digest`/`signature`/`branding`/`RenderSpec`, which are added at seal),
  `Justification` / `JustificationKind`, `Risk` (axes/weights/total/band —
  **integer** arithmetic, relenor weights), `Category`, `Lifecycle`,
  `Visibility`, `KnownIssue`, `RenderSpec`, `Branding`, `DataStructureChange`,
  `ReleaseKey`.
- Canonicalizer: pick and **pin** one (candidates: `json-canon` 0.1,
  `serde_json_canonicalizer` 0.3, `serde_jcs` 0.2 — §14 flags `serde_jcs` as
  under-maintained). The canonical bytes are a **cryptographic contract**.
- Digest excludes the envelope (`digest`/`signature` are outside the hashed
  body).
- **Tests:** a **byte-level golden** canonical-JSON assertion over a fixed
  fixture manifest (locks the contract); digest stability + digest-excludes-
  itself; risk total/band at boundaries; serde round-trips; kebab-case enums.
- **Smell test:** one capability (deterministic manifest model + digest), pure,
  fully unit-tested; revertable. **~550 LOC.**

### 2.2 — `crt: sign and verify release manifests with detached OpenPGP`

**After this:** `crt-core` can produce a detached, armored OpenPGP signature
over canonical manifest bytes and verify one — the authenticity anchor.

- **Begins with an rPGP 0.19 API spike** against the registry source (confirm
  armored `SignedSecretKey`/`SignedPublicKey` parsing and detached create/verify
  call shapes) before finalizing the API — as was done for octocrab.
- `crt-core`:
  `sign_manifest(canonical_bytes, secret_key_armored) -> ArmoredSignature`
  (detached) and
  `verify_manifest(canonical_bytes, signature, public_key_armored) -> Result<()>`.
  Pure compute over in-memory bytes.
- **Tests:** generate a test keypair in-test; sign → verify roundtrip; tampered
  canonical bytes ⇒ verify fails; wrong key ⇒ verify fails.
- **Smell test:** the authenticity capability over 2.1's canonical bytes; pure,
  unit-tested. **~350 LOC.** (Could merge with 2.1 if either underruns; kept
  split so integrity and authenticity are separately revertable.)

### 2.3 — `crt: persist drafts, sealed releases, and templates in the store`

**After this:** the store reads/writes mutable **drafts**, write-once
`ReleaseRecord`s, and notes templates, and lists both; a sealed key cannot be
silently overwritten.

- `crt-store`: extend `Store` with mutable draft CRUD
  (`put_draft`/`get_draft`/`list_drafts`/`delete_draft`) **and** write-once
  release ops (`put_release`/`get_release`/`list_releases`) plus
  `put_template`/`get_template`; `ReleaseKey`; key layout
  (`drafts/<ns>/<channel>/<name>.json` — **mutable, overwritable**;
  `releases/<ns>/<channel>/<name>.json` — **write-once**;
  `templates/sha256/<digest>`); **write-once guard** via `object_store`
  `PutMode::Create` (atomic — no TOCTOU gap), so `put_release` of an existing
  key is refused (design §5); `put_draft` is freely overwritable (serial
  handoff). **`list` enumerates the prefix** (the source of truth); design §5's
  `index/releases.json` is a re-derivable cache, deferred to the service era
  (avoids write-amplification and the read-modify-write race a maintained index
  would add).
- **Tests:** round-trip draft + release + template via `InMemory` and
  `LocalFileSystem`; draft overwrite succeeds; release write-once guard rejects
  an overwrite; `list_drafts`/`list_releases` reflect their keys. No minio
  (real-S3 stays the opt-in `#[ignore]`d test).
- **Smell test:** the draft + sealed-release persistence capability,
  round-trip-tested. **~500 LOC.**

### 2.4 — `crt: author a draft release (release new/add/info)`

**After this:** an operator can create a draft from imported blobs **in the
shared store**, add entries with metadata, and inspect the draft — and a second
operator with store access can pick it up.

- `crt`: config additions **consumed here** — `namespaces`/`channels`/`branding`
  and `risk_components` (channel resolution + risk component validation).
- `release new <name>` (prefix-resolve `name` → namespace/channel; `put_draft` a
  new draft into the store); `release add <name> <blob_hash…>` (`get_draft` →
  append entries → `put_draft`; `--category --visibility --justification --ref`;
  `$EDITOR` for `public_summary` / `behavior_change` / `upgrade_notes`);
  `release info <name>` reads the draft (or, if none, the sealed release) from
  the store so the capability is observable, not write-only.
- **Tests:** name→channel resolution; draft create → `add` → re-read round-trips
  through the store (incl. an inert `private` entry); `info` renders a draft.
- **Smell test:** author + observe a shared draft; the first real consumer of
  the 2.1 types. **~600 LOC.**

**Deviations from §10, recorded as landed:**

- **Clobber-guard on `new`.** `release new` refuses if a draft **or** sealed
  release already exists for the resolved key, rather than blindly
  `put_draft`-ing. A blind overwrite would silently wipe a colleague's
  in-progress draft — unacceptable in the store-backed, collaborative model that
  motivated putting drafts in the shared store. A `--force` reset is deferred.
- **`release new --base-ref <ref>` is required.** `ReleaseHeader.base_ref`
  cannot be reliably derived from the name (`ces-v18.2.0-clyso1` ≠ base
  `v18.2.0`), so it is an explicit flag. The author defaults to
  `git config user.{name,email}`, overridable via `--author-name`/
  `--author-email`.
- **`release add` takes narrative flags.** `--public-summary` /
  `--behavior-change` / `--upgrade-notes` are a scriptable, testable superset of
  the design's "`$EDITOR` for …"; `$EDITOR` opens (composing all three in one
  buffer) only when `--public-summary` is omitted. Risk axes are flags
  (`--component` validated against `risk_components`, `--blast`/`--conflict`/
  `--coverage`); duplicate blobs are skipped (idempotent re-runs); a blob with
  no stored metadata is an error.
- **`info` is observable for drafts and sealed releases.** It reads the draft,
  falling back to the sealed release (via `StoreError::is_not_found`), so the
  capability is not write-only.
- Design §10 still literally reads "Draft releases live locally until seal";
  that is **superseded** by this plan's store-backed-drafts decision (above) —
  the authoritative design doc has not yet been amended.

### 2.5 — `crt: seal a draft into a signed release (release seal/list)`

**After this:** `crt release seal <name>` turns a draft into a signed, persisted
`ReleaseRecord`; `crt release list` shows sealed releases.

- `crt`: `vault` secrets block + a thin `vaultrs` private-key fetch (edge shim);
  ship the **default notes template asset** and `put_template` it at seal (see
  approval gate — sealed but **not rendered/validated** until M3).
- `release seal`: `get_draft` → compute risk bands → snapshot `branding` →
  record `RenderSpec` (minijinja version + template digest) + `put_template` →
  canonicalize → digest → **fetch key from Vault → sign →** `put_release`
  (write-once) → `delete_draft` **LAST** (only after the sealed record lands).
  Ordering is load-bearing: a Vault/sign failure must not burn the write-once
  key with a half-sealed record, and the draft is removed only once sealing
  succeeds (so a failed seal leaves the draft intact for retry/handoff).
- `release list` (sealed releases, from the store index).
- **Tests:** seal pipeline with **injected key bytes** (pure path; no Vault); a
  separate `#[ignore]`d Vault-fetch test (env-gated, no HTTP-mock dep); `list`.
- **Smell test:** seal + sign + persist; query. **~600 LOC.**

**Decisions, recorded as landed:**

- **RenderSpec sequencing — honored option (a).** Seal snapshots `branding`,
  records `RenderSpec`, and `put_template`s the default notes asset. The
  template (`crt/assets/default-release-notes.md.j2`) is **sealed but not
  rendered/validated until M3**; `RenderSpec.minijinja_version` is a
  **provisional constant** (`2.5.0`) because `minijinja` is not yet linked — M3
  pins the real version, and both it and the template digest may shift then (no
  production release is sealed in between).
- **Two write-once seal guards** (beyond the planned ordering): seal **refuses
  an empty draft** (a zero-entry signed manifest is almost always a forgotten
  `add`), and **requires branding to resolve** from the draft's _stored_
  `namespace`/`channel` (not a re-resolution of the name — config may have
  drifted) — sealing empty branding into a signed manifest is permanent, so a
  missing channel is a hard error. A cheap `get_release` pre-check refuses
  before the Vault round-trip; `put_release` remains the authoritative
  write-once guard.
- **Vault edge shim** (`crate::vault`): `vaultrs` 0.8 with
  `default-features = false, features = ["rustls"]` — **verified
  C-dependency-free** (no `openssl-sys`/`native-tls`), matching the
  object_store/octocrab rustls stack. The signing secret carries a `private-key`
  field (+ optional `passphrase`), matching cbscore's
  `GPGVaultPrivateKeySecret`. The path string is split into mount + KV-v2 path
  (the `data` infix is dropped — `vaultrs::kv2::read` re-inserts it). The live
  fetch is an `#[ignore]`d, env-gated test; `split_kv2_path` is unit-tested.
- **`crt-core::SCHEMA_VERSION`** is now the single source of truth the seal
  stamps and the 2.1 fixture references, so 2.6's verify cannot drift from it.
- The planned **"compute risk bands"** step is a **no-op at seal**: bands are
  computed-not-stored (the 2.1 invariant), so there is nothing to compute into
  the manifest, and `release info` already surfaces each entry's
  `risk_total`/`risk_band` for pre-seal inspection.

### 2.6 — `crt: verify a sealed release (signature, schema, cross-reference)`

**After this:** `crt release verify <name>` runs §11 legs 0–2 and reports
clearly that legs 3–4 are not yet applicable.

- `crt`: `public_key_url` config + a thin public-key fetch (a local path is
  accepted for tests). `release verify`: **leg 0** signature (verify
  `ReleaseRecord.signature` over the canonical manifest with the public key);
  **leg 1** schema (deserialize/validate record + manifest + referenced
  `PatchMeta`); **leg 2** cross-reference (recomputed digest == stored; entry
  `blob_hash` set == referenced blobs; every referenced blob exists). Legs 3–4
  **explicitly reported as skipped** ("no materialized ref — M3/M4"), never
  silently passed. Distinct exit codes for signature vs verify vs operational
  failure.
- **Tests:** a sealed release verifies; a tampered manifest ⇒ signature failure;
  a missing referenced blob ⇒ cross-reference failure; the skipped-legs notice
  is emitted.
- **Smell test:** the verify capability over a sealed release. **~450 LOC.**

## Progress

| Commit                                   | Status  | Notes                                               |
| ---------------------------------------- | ------- | --------------------------------------------------- |
| 2.1 manifest model + canonical digest    | ✅ done | pure crt-core; byte-level golden test               |
| 2.2 detached OpenPGP sign/verify         | ✅ done | rPGP 0.19, no-default-features (no C dep)           |
| 2.3 store: drafts + releases + templates | ✅ done | mutable drafts; write-once releases; list-by-prefix |
| 2.4 draft authoring (new/add/info)       | ✅ done | channel config; clobber-guarded store-backed drafts |
| 2.5 seal (Vault + sign) + list           | ✅ done | delete_draft LAST; key bytes injected; rustls Vault |
| 2.6 verify (legs 0–2)                    | ☐ todo  | legs 3–4 reported skipped                           |

(Update after each commit lands.)

## Approval-gate questions

1. **RenderSpec / branding / template sequencing.** The design (§10/§13) seals
   the branding snapshot, `RenderSpec`, and the stored notes **template** at
   `seal` time (M2), with notes **rendering** in M3. Honoring it means M2 ships
   the default template as an asset and stores it, but the template is
   **sealed-but-unrendered (thus unvalidated) until M3, and its digest may shift
   then** — safe **only** because no production release is sealed between M2 and
   M3 landing. Options: **(a, recommended)** honor the design — seal
   branding+RenderSpec+template in M2; **(b)** defer all three to M3, so M2
   manifests omit them and the manifest schema grows in M3.
2. **Draft storage location/format** — **RESOLVED: store-backed.** Local-only
   drafts strand the work (no second operator, no continuity if the author is
   unavailable), so drafts are **mutable objects in the shared store** under a
   `drafts/` prefix, separate from write-once `releases/` (see "Drafts live in
   the shared store" above). MVP concurrency is serial handoff
   (last-writer-wins); optimistic concurrency / the GitHub-Issues workflow is
   deferred.
3. **Canonicalizer choice** (non-blocking; pinned in 2.1). Lean `json-canon` if
   actively maintained, else `serde_json_canonicalizer`; `serde_jcs` only if
   neither fits (§14 flags it under-maintained). Locked by the 2.1 golden test
   regardless of choice.

## Verification (M2 end-to-end)

1. Gate per `cbsd-rs/CLAUDE.md`: `cargo fmt --all`, `cargo clippy --workspace`,
   `cargo check --workspace`; `cargo test --workspace` green **without** Vault,
   minio, or network.
2. `crt release new <name>` → `add` (mixed `public`/`private`, with risk +
   justification) → `seal` (signs with an injected/Vault key) → `verify` passes
   (digest matches, signature valid); `info`/`list` show the release.
3. Tamper checks: mutating the stored manifest ⇒ `verify` signature failure;
   deleting a referenced blob ⇒ cross-reference failure; re-sealing an existing
   key ⇒ write-once guard refuses.

## Notes / risks

- **rPGP 0.19 API** is the top implementation risk — spike before 2.2 (above).
- **Canonical bytes are a signed contract** — the 2.1 golden test guards against
  a canonicalizer swap silently invalidating signatures; prefer integer risk
  weights to avoid RFC 8785 number-canonicalization float edges.
- **`crt-core` purity** — signing/verification take key bytes; no Vault/IO in
  `crt-core`. Vault (`vaultrs` 0.8) and the public-key fetch are `crt`-only edge
  shims; `cargo test` never touches Vault or the network.
- **Config additions land with their first consumer** (no sealed-in-config that
  nothing reads): channels/branding/`risk_components` → 2.4; `vault` → 2.5;
  `public_key_url` → 2.6; `destination_repo` deferred to M4 (materialize).
- New deps arrive with their consumer: canonicalizer + `pgp` 0.19 (2.1/2.2,
  `crt-core`); `vaultrs` 0.8 (2.5, `crt`). `minijinja` is an M3 dep.
- **Draft concurrency** — store-backed drafts are mutable; the MVP is
  last-writer-wins (serial handoff), so two simultaneous `add`s to one draft can
  lose an update. Mitigation (conditional puts via `object_store` update
  versions, or the GitHub-Issues workflow) is deferred; flagged so it is not
  mistaken for safe concurrent editing.
- **Short-hash patch selection** in `release add` (§14) — deferred nicety.
