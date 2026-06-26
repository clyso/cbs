# CRT v2 — implementation status & handoff

> Operational status snapshot for CRT v2 (the Ceph Release Tool). Updated at the
> end of M3. Not subject to the `seq-docs-convention` naming (operational file).
> **Last updated:** 2026-06-25 (M3 + v5 review), on branch
> `wip/release-tool-v2`.

## What CRT v2 is

A downstream Ceph release patch-management tool: ingest patches into a
content-addressed store, compose a release manifest, **seal** it (canonical JSON
→ sha256 digest → detached OpenPGP signature), and **verify** a sealed release.
Three crates in the `cbsd-rs/` workspace (edition 2024, GPL-3.0-or-later):

- **`crt-core`** — pure domain logic, **no IO / no tokio**: manifest model, risk
  rubric, RFC 8785 canonical JSON + digest, detached OpenPGP sign/verify. Signs
  and verifies over **in-memory key bytes** — never touches Vault or the
  network.
- **`crt-store`** — `object_store`-backed persistence (`InMemory` for tests,
  `LocalFileSystem` for dev, `AmazonS3` for prod). Blobs, patch meta, drafts,
  write-once sealed releases, notes templates.
- **`crt`** — the clap CLI. Owns the IO edge shims (subprocess `git`, `octocrab`
  for PR metadata, `vaultrs` for the signing key, `reqwest` for the public key).

## Authoritative documents

| Doc                                                                | What                                 |
| ------------------------------------------------------------------ | ------------------------------------ |
| `docs/crt/000-concept.md`                                          | Concept / rationale                  |
| `docs/crt/design/001-20260620T1318-v2-mvp.md`                      | **Authoritative design** (MVP)       |
| `docs/crt/plans/001-20260621T0930-01-store-and-import.md`          | M1 plan                              |
| `docs/crt/plans/001-20260621T2212-02-manifest-seal-sign-verify.md` | M2 plan (progress table + decisions) |
| `docs/crt/plans/001-20260623T1717-03-sbom-notes-materialize.md`    | M3 plan (progress table + decisions) |
| `docs/crt/reviews/001-*-impl-v2-mvp-v{1,2}.md`                     | M1 reviews                           |
| `docs/crt/reviews/001-20260622T0515-impl-v2-mvp-v3.md`             | M2 commits 2.1–2.3 review            |
| `docs/crt/reviews/001-20260622T2040-impl-v2-mvp-v4.md`             | M2 commits 2.4–2.6 review (GO/80)    |

If code and design disagree, **fix the code** — but several intentional
deviations from the design are recorded in the M2 plan's per-commit "Decisions"
blocks (store-backed drafts; required `--base-ref`; narrative flags as an
`$EDITOR` superset; the `new` clobber-guard; the two seal guards; the
provisional `RenderSpec.minijinja_version`; the corrupt-object→exit-1
simplification of §11 leg 1; distinct verify exit codes; `https`-only public-key
fetch). Treat those as authoritative-as-landed.

## Milestone status

| Milestone | Scope                                                                 | Status                    |
| --------- | --------------------------------------------------------------------- | ------------------------- |
| **M1**    | Patch ingestion into a content-addressed store                        | ✅ done                   |
| **M2**    | Sealed, signed manifests + `verify` legs 0–2                          | ✅ done + reviewed        |
| **M3**    | Deterministic SBOM (§7.1) + notes (§7.2) + `materialize` artifacts    | ✅ done                   |
| **M4**    | `materialize` (git ref/tag + signed `000-RELEASE/`) + `verify --tree` | 🔶 4.1–4.3 done; 4.4 todo |

### M1 — done (`3a0cbe4e`, `f87ac939`, `30d09904`)

`crt patch import <--repo --range A..B | --pr <url>>`: patch bytes from a local
`git format-patch` (reproducible blob hash), `patch_id` via
`git patch-id --stable`, `PatchMeta` written alongside; `patch_id` equivalence
is flagged. S3 store backend + `crt.config.yaml` / `crt.secrets.yaml` loading.

### M2 — done + reviewed (`38422608`, `c5a7f29c`, `720b2c0a`, `1ccd8554`, `fe83c7df`, `abb67671`)

(SHAs as of 2026-06-23, post-autosquash; subjects are the stable reference.)

- **2.1** `crt: model release manifests with a canonical-JSON digest` — pure
  `crt-core` manifest model, RFC 8785 canonical JSON
  (`serde_json_canonicalizer`), sha256 digest (digest-excludes-itself), integer
  risk rubric. Byte-level golden test locks the canonical contract.
- **2.2** `crt: sign and verify release manifests with detached OpenPGP` — `pgp`
  (rPGP) 0.19, `default-features = false`; detached armored signature over the
  canonical bytes; RNG injected (crt-core stays pure).
- **2.3** `crt: persist drafts, sealed releases, and templates in the store` —
  mutable `drafts/`, **write-once** `releases/` (atomic `PutMode::Create`),
  `templates/sha256/`; `list` enumerates the prefix (no maintained index).
- **2.4** `crt: author a draft release (release new/add/info)` — name→channel
  prefix resolution, risk-component validation, `new` clobber-guard, `add`
  (flags + `$EDITOR`), `info`.
- **2.5** `crt: seal a draft into a signed release (release seal/list)` — Vault
  key fetch (edge shim) → sign → write-once `put_release` → `delete_draft` last;
  empty-draft + missing-branding guards; `list`.
- **2.6** `crt: verify a sealed release (signature, schema, cross-reference)` —
  §11 legs 0–2; legs 3–4 reported **skipped**; distinct exit codes (2 signature,
  3 verify, 1 operational); `https`-or-local public-key fetch.

**Verified end-to-end today:** `import → release new → add → seal → verify`,
plus `list` / `info`. Gate green: `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets`, `cargo test --workspace` (no Vault, no
minio, no network — live S3/Vault/URL paths are `#[ignore]`d).

### M3 — done (commits 3.1–3.2; subjects are the stable reference)

- **3.1** `crt: render release notes from the sealed manifest` — `minijinja`
  linked in `crt-core` (exact `=2.21.0`); `RENDER_MINIJINJA_VERSION` moved into
  `crt-core` and corrected `"2.5.0"` → `"2.21.0"` (matching the 2.1 golden
  fixture — so the golden was **undisturbed**). Pure `render_notes`
  (`trim_blocks`/`lstrip_blocks`; the default template re-expressed for
  minijinja's `groupby`-as-filter). `release notes <name>` is **sealed-only**
  and **version-gated**; renders `public_summary` and strips
  `justification.internal` from the template context — structural, not by
  template convention (v5 F4).
- **3.2**
  `crt: derive a deterministic CycloneDX SBOM and emit release artifacts` —
  hand-rolled CycloneDX 1.6 (`crt-core::build_sbom`): one Ceph component, each
  patch under `pedigree.patches[]`; `serialNumber` from the manifest digest,
  `timestamp` from `release.created` (no random/wall-clock).
  `release materialize <name> --out <dir>` emits `RELEASE-NOTES.md` +
  `sbom.cdx.json`; determinism locked by a committed byte golden + a
  re-build-equality test (v5 F1).

**Verified end-to-end (M3):** seal → `release notes` renders the pinned template
(internal note hidden); `release materialize` writes both artifacts and a re-run
is byte-identical. Gate green: `cargo fmt --all --check`,
`cargo clippy -p crt -p crt-core --all-targets`, `cargo test` (crt 44, crt-core
26, crt-store 12), `cargo check --workspace`.

## M4 — in progress (4.1–4.3 done; 4.4 remaining)

Git materialization and the portable signed bundle (design §8, §11 legs 3–4).
See `docs/crt/plans/001-20260625T0831-04-git-materialize-verify-tree.md` for the
full plan + per-commit progress table.

- **4.1 done (`5dbeea3`)** — `crt release materialize` builds the linear
  `release/<name>` branch (`git am` per entry, each amended with a `Crt-Patch`
  trailer) in a clean checkout of the destination repo (`core.autocrlf=false`).
- **4.2 done (`e359ecc`)** — `source_tree_digest` (canonical directory hash,
  §14) and offline `crt verify --tree <dir>` (no store/git): signature +
  `source_tree_digest` + exhaustive `bundle_digests`.
- **4.3 done** — `materialize` appends the signed `000-RELEASE/` bundle commit
  (`record.json` + detached `.asc`, `sbom.cdx.json`, `RELEASE-NOTES.md`,
  `provenance.json`, `README.md`, `.gitattributes`) and an annotated tag
  carrying the manifest digest; opt-in `--push`. `materialize` now needs the
  Vault key (it signs the bundle).
- **4.4 todo** — activate the ref-conditional `release verify` legs 0–4 (bundle
  signature, in-tree record schema/cross-ref, git anchoring via `Crt-Patch` +
  `git patch-id --stable`, and leg-4 byte-compare of `sbom.cdx.json` /
  `RELEASE-NOTES.md` against an M3 re-derivation). **Pin `serde_json` (exact)**
  with leg 4 (v5 F2): the M3 SBOM byte golden catches a pretty-printer shift,
  but leg 4's byte-compare wants the renderer pinned too, mirroring the
  `minijinja` exact pin on the notes side.

The `RenderSpec.minijinja_version` reconciliation is **done**:
`minijinja 2.21.0` is linked and exact-pinned in `crt-core`, and
`RENDER_MINIJINJA_VERSION` (now owned by `crt-core`) is the single source of
truth — seal stamps it, `release notes` gates on it, and leg 4 (M4) will too.

## Carry-forward invariants (do not regress)

- **`crt-core` purity:** no IO, no tokio. Signing/verification take key bytes;
  Vault (`vaultrs` 0.8, `rustls`) and the public-key fetch (`reqwest` 0.12,
  `rustls-tls`) are `crt`-only edge shims. **No `openssl`/`native-tls`** in the
  tree — keep it that way (verified via `cargo tree -i openssl-sys`).
- **Canonical bytes are a signed contract.** The 2.1 byte-level golden test
  guards against a canonicalizer swap silently invalidating signatures. Prefer
  integer risk weights (no float canonicalization edges).
- **`crt-core::SCHEMA_VERSION`** is the single source of truth that seal stamps
  and verify checks — keep both sides referencing it.
- **Write-once + ordering:** `put_release` is the authoritative write-once
  guard. Seal order is load-bearing: sign **before** `put_release`;
  `delete_draft` **last**. `release new` and the CLI `seal` arm pre-check before
  pulling the private key.
- **Branding is snapshotted at seal** into the manifest; notes render from that
  snapshot, never from live `crt.config.yaml`.
- **Vault signing secret** convention: a `private-key` field (armored) +
  optional `passphrase`, matching cbscore's `GPGVaultPrivateKeySecret`.

## Deferred / known backlog

- **SBOM not run through a CycloneDX validator** (§7.1/§14): M3 tests the shape
  structurally (parse + key fields) and checks `serialNumber` against the spec
  regex, but no validator is on PATH here. Run one in M4 and, with it, refine
  issue `references` (a bare `CVE-2026-0001` wants `iri-reference` form, or
  belongs in the issue `id`).
- **F2 (documented deviation):** a corrupt stored `ReleaseRecord`/`Manifest` (or
  `PatchMeta`) surfaces as operational **exit 1**, whereas §11 leg 1 frames a
  deserialize failure as a verify failure (**exit 3**). A _missing_ release/meta
  is handled correctly. Revisit if exit-3 for corrupt objects is wanted.
- **CLI seal→verify chain needs a live Vault** for `seal`, so that wiring is
  exercised only by in-process tests; the Vault / S3 / public-key-URL edges have
  `#[ignore]`d env-gated tests (`CRT_TEST_VAULT_*`, `CRT_TEST_S3_*`,
  `CRT_TEST_PUBKEY_URL`).
- **`index/releases.json`** is deferred (design §5) — `list` enumerates the
  store prefix as the source of truth.
- **Draft concurrency** is last-writer-wins (serial handoff). Optimistic
  concurrency (conditional puts) or the GitHub-Issues workflow (concept §8) is
  deferred — not safe for simultaneous editing of one draft.
- **Short-hash patch selection** in `release add` (§14) — deferred; full 64-char
  blob hashes only for now.
- **`visibility`** is recorded but **inert** (design §7) — no redaction;
  enforcement deferred to a future service gateway.
- **`data_structure_change`** (concept §6.4) has no CLI setter yet (defaults
  `None`); **cross-release lifecycle** (`first_shipped_in`) is not tracked
  (entries seal `status: active`, `first_shipped_in: null`).
- **Key rotation / revocation / fingerprint pinning** (design §6.1/§14) and
  **cosign/sigstore** (§6.2) are open operational/future items.

## Dev workflow & gate

- Branch: `wip/release-tool-v2`. Commit prefix: `crt:` (docs: `crt/docs:`).
- Pre-commit gate (run from `cbsd-rs/`): `cargo fmt --all`,
  `cargo clippy --workspace`, `cargo check --workspace`; `SQLX_OFFLINE=true`
  when no DB. `cargo test --workspace` before declaring a milestone done.
- Commits: DCO sign-off (`-s`), **never GPG-sign autonomously**
  (`--no-gpg-sign`), exactly one `Co-authored-by` trailer. Separate `git add` /
  `git commit`; no compound `cd && …`, no `git -C`, no `git -c`.
- Review cadence: implement a milestone's commits → adversarial review of the
  group (`pr-review-toolkit:code-reviewer` via the `adversarial-review` skill,
  with `confidence-scoring` + `seq-docs-convention`) → address findings via
  `git commit --fixup=<introducing-commit>` → review doc as a standalone commit
  → **user runs the autosquash**
  (`GIT_SEQUENCE_EDITOR=true git rebase --no-gpg-sign --autosquash <parent>`).
- Docs live under `cbsd-rs/docs/crt/` per `seq-docs-convention`; wrap at 79 cols
  and format with `prettier --write <path>`.
