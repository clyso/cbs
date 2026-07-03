# CRT v2 — implementation status & handoff

> Operational status snapshot for CRT v2 (the Ceph Release Tool). Not subject to
> the `seq-docs-convention` naming (operational file). **Last updated:**
> 2026-07-03 (MVP complete; post-MVP patch introspection (seq-002) and patch
> annotations (seq-003) landed), on branch `wip/release-tool-v2`.

## Session handoff — read first

Written for the next session and its agents; skim this, then follow the
cross-referenced sections. Human readers: the narrative starts at "What CRT v2
is".

**Position.** MVP (M1–M4) plus two post-MVP features — seq-002 (patch
introspection) and seq-003 (patch annotations, review GO/94) — are landed,
reviewed, and gate-green (`cargo fmt --all --check`;
`cargo clippy --workspace --all-targets` = 0 warnings; `cargo test --workspace`,
crt 114 / crt-core 41 / crt-store 14). Nothing is broken or half-done; there is
no in-flight change to resume.

**Branch / remote.** Local history was rewritten (fixup autosquashes) with new
commits on top, so it has **diverged from the remote** — a force-push is needed
before the remote matches (operator's call). Do **not** merge to `main`: the
feature set is WIP by design.

**Recommended next task — the `release add` applicability guard** (seq-003 §10;
first item under "Deferred / known backlog"). Make `release add` reject or warn
when a patch's seq-003 `applies_to` excludes the draft's `base_ref`:

- The draft carries `ReleaseHeader.base_ref` (a version string, e.g. `v18.2.0`).
- Reuse `crt_core::parse_version_query` + `Applicability::matches` /
  `applies_to_matches` — the same §7 matching the seq-003 filters use. `Generic`
  matches everything; `None` (unassessed) matches nothing — decide at design
  time whether unassessed is a warn or a hard block.
- Read each blob's record via `store.get_annotations` in the `release add` path
  (`crt/src/release.rs` + the `Add` arm in `crt/src/main.rs`).
- Give it its own seq-004 `seq-docs` trail: design → plan → implement → review.

Lower-priority forward work and longer-horizon direction are enumerated under
"Deferred / known backlog".

**Working agreements an agent MUST honor** (in addition to "Dev workflow &
gate"):

- **Commit messages via `-F _local/<name>.txt` only — never repeated `-m`**
  (standing user directive; `-m` misbehaves). `_local/` is gitignored. Keep
  deny-glob words
  (fetch/push/pull/rebase/reset/checkout/restore/remote/tag/worktree) out of the
  `-F` **path**; they are fine inside the message body (with `-F` the body never
  reaches the command line).
- **The operator runs `git rebase --autosquash`, not the agent** (`rebase` is
  shell-guard-blocked for agents). Agents create `--fixup` commits and hand over
  the autosquash command.
- Docs under `cbsd-rs/docs/**`: format with `npx prettier@3.9.1 --write <path>`
  (79-col); never manually wrap; never run markdownlint with `fix` (it rewrites
  markdown repo-wide).

**Environment gotchas (transient).**

- **GitNexus MCP is stale/broken** this session
  (`FTS … Database file version: 41, Current build storage version: 40`):
  `impact` / `detect_changes` / `query` fail, so CLAUDE.md's "MUST run impact
  analysis before editing a symbol" cannot be honored — fall back to `cargo` +
  `grep`, and scope edits by reading callers directly. Re-index with
  `node .gitnexus/run.cjs analyze` if the graph is needed.
- A stray plan-mode file titled "wrap `crt --help` output" is **already
  implemented** (commit `815e3a6`: `wrap_help` + `max_term_width` + slimmed
  subcommand summaries) — treat any such plan as stale.

**Extending annotations?** The load-bearing contracts live in
`design/003-…-patch-annotations-and-list-views.md`: the flag→state transitions
live in `crt/src/annotate.rs`, **not** `crt-core` (§9 keeps core to types +
matching); `applies_to = None` is never treated as `Generic`; `import`
**merges** annotations and never clobbers; and the `patch list --json`
`{meta, annotations}` element is the single pre-stable breaking change.

## What CRT v2 is

A downstream Ceph release patch-management tool: ingest patches into a
content-addressed store, compose a release manifest, **seal** it (canonical JSON
→ sha256 digest → detached OpenPGP signature), and **verify** a sealed release.
Three crates in the `cbsd-rs/` workspace (edition 2024, GPL-3.0-or-later):

- **`crt-core`** — pure domain logic, **no IO / no tokio**: manifest model, risk
  rubric, RFC 8785 canonical JSON + digest, detached OpenPGP sign/verify,
  patch-annotation types + ceph-version matching (seq-003). Signs and verifies
  over **in-memory key bytes** — never touches Vault or the network.
- **`crt-store`** — `object_store`-backed persistence (`InMemory` for tests,
  `LocalFileSystem` for dev, `AmazonS3` for prod). Blobs, patch meta, operator
  annotations (`patches/annotations/`, seq-003), drafts, write-once sealed
  releases, notes templates.
- **`crt`** — the clap CLI. Owns the IO edge shims (subprocess `git`, `octocrab`
  for PR metadata, `vaultrs` for the signing key, `reqwest` for the public key).

## Authoritative documents

| Doc                                                                     | What                                 |
| ----------------------------------------------------------------------- | ------------------------------------ |
| `docs/crt/000-concept.md`                                               | Concept / rationale                  |
| `docs/crt/design/001-20260620T1318-v2-mvp.md`                           | **Authoritative design** (MVP)       |
| `docs/crt/plans/001-20260621T0930-01-store-and-import.md`               | M1 plan                              |
| `docs/crt/plans/001-20260621T2212-02-manifest-seal-sign-verify.md`      | M2 plan (progress table + decisions) |
| `docs/crt/plans/001-20260623T1717-03-sbom-notes-materialize.md`         | M3 plan (progress table + decisions) |
| `docs/crt/plans/001-20260625T0831-04-git-materialize-verify-tree.md`    | M4 plan (progress table + decisions) |
| `docs/crt/reviews/001-*-impl-v2-mvp-v{1,2}.md`                          | M1 reviews                           |
| `docs/crt/reviews/001-20260622T0515-impl-v2-mvp-v3.md`                  | M2 commits 2.1–2.3 review            |
| `docs/crt/reviews/001-20260622T2040-impl-v2-mvp-v4.md`                  | M2 commits 2.4–2.6 review (GO/80)    |
| `docs/crt/reviews/001-20260625T0449-impl-v2-mvp-v5.md`                  | M3 review                            |
| `docs/crt/reviews/001-20260626T0919-impl-v2-mvp-v6.md`                  | M4 review (GO/80)                    |
| `docs/crt/design/002-20260627T1645-patch-list-info.md`                  | seq-002 design (patch list/info)     |
| `docs/crt/plans/002-20260627T1659-patch-list-info.md`                   | seq-002 plan                         |
| `docs/crt/design/003-20260628T0807-patch-annotations-and-list-views.md` | seq-003 design (annotations)         |
| `docs/crt/reviews/003-*-design-…-v{1,2,3}.md`                           | seq-003 design reviews               |
| `docs/crt/reviews/003-20260629T0758-impl-…-v1.md`                       | seq-003 impl review (GO/94)          |

If code and design disagree, **fix the code** — but several intentional
deviations from the design are recorded in the M2 plan's per-commit "Decisions"
blocks (store-backed drafts; required `--base-ref`; narrative flags as an
`$EDITOR` superset; the `new` clobber-guard; the two seal guards; the
provisional `RenderSpec.minijinja_version`; the corrupt-object→exit-1
simplification of §11 leg 1; distinct verify exit codes; `https`-only public-key
fetch). Treat those as authoritative-as-landed.

## Milestone status

| Milestone | Scope                                                                 | Status                         |
| --------- | --------------------------------------------------------------------- | ------------------------------ |
| **M1**    | Patch ingestion into a content-addressed store                        | ✅ done                        |
| **M2**    | Sealed, signed manifests + `verify` legs 0–2                          | ✅ done + reviewed             |
| **M3**    | Deterministic SBOM (§7.1) + notes (§7.2) + `materialize` artifacts    | ✅ done                        |
| **M4**    | `materialize` (git ref/tag + signed `000-RELEASE/`) + `verify --tree` | ✅ done + reviewed (v6: GO/80) |

### Post-MVP features (on top of the MVP design 001)

Two features landed after the MVP, each with its own `seq-docs`
design/plan/review trail:

- **seq-002 — patch introspection (done).** `crt patch list` and
  `crt patch info` read the content-addressed store: full or **short-hash** blob
  lookup, text or `--json`.
- **seq-003 — patch annotations & richer list views (done, GO/94).** An
  operator-authored annotations record per blob — applicability (`Generic` vs a
  set of ceph-version `Line`/`Exact` specs), free-form tags, a description, and
  an open attribute bag — kept **separate** from the git-derived `PatchMeta`
  (`patches/annotations/sha256/<blob>.json`) and **merged** across re-imports,
  never clobbered. Set in bulk at `import`
  (`--ceph-version`/`--generic`/`--tag`) or per patch via `crt patch annotate`.
  `patch list` gains `--group-by pr|source-repo|ceph-version|tag`, the
  `--ceph-version`/`--tag`/`--unassessed` filters, and an annotations column;
  `--json` elements become `{meta, annotations}`.

Also post-M4: private-repo PR import, release `--push`, and the public-key fetch
now authenticate off-argv with a GitHub token; `crt --help` wraps to the
terminal with slimmed subcommand summaries.

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

## M4 — done + reviewed

Git materialization and the portable signed bundle (design §8, §11 legs 0–4).
All four commits have landed, the M4-group adversarial review is complete
(`docs/crt/reviews/001-20260626T0919-impl-v2-mvp-v6.md` — **GO, 80/100**, both
findings addressed), and the fixups have been autosquashed. **This completes the
MVP: nothing in the design (`design/001-20260620T1318-v2-mvp.md`) remains
unbuilt.** See
`docs/crt/plans/001-20260625T0831-04-git-materialize-verify-tree.md` for the
full plan + per-commit progress table; the post-MVP backlog is below.

- **4.1 done** — `crt release materialize` builds the linear `release/<name>`
  branch (`git am` per entry, each amended with a `Crt-Patch` trailer) in a
  clean checkout of the destination repo (`core.autocrlf=false`).
- **4.2 done** — `source_tree_digest` (canonical directory hash, §14) and
  offline `crt verify --tree <dir>` (no store/git): signature +
  `source_tree_digest` + exhaustive `bundle_digests`.
- **4.3 done** — `materialize` appends the signed `000-RELEASE/` bundle commit
  (`record.json` + detached `.asc`, `sbom.cdx.json`, `RELEASE-NOTES.md`,
  `provenance.json`, `README.md`, `.gitattributes`) and an annotated tag
  carrying the manifest digest; opt-in `--push`. `materialize` now needs the
  Vault key (it signs the bundle).
- **4.4 done** — `crt release verify --repo` runs the ref-conditional legs 0–4
  when the release is materialized: bundle signature, in-tree record
  schema/cross-ref (back-ref + BOM faithfulness), git anchoring (leg 3:
  `Crt-Patch` trailer + offset-invariant `git patch-id --stable` recomputed from
  the commit diff), and artifact faithfulness (leg 4: SBOM/notes byte-compare to
  a sealed re-derivation). `VerifyVerdict` now carries the per-leg report
  (`LegState::Failed`, F9); `serde_json` pinned exact in `crt-core` (F2).

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
- **Short-hash patch selection**: `patch info` / `patch annotate` resolve a
  unique short prefix (seq-002/003); `release add` is still full-64-char only
  (§14).
- **`release add` applicability guard** — reject or warn when a patch's seq-003
  `applies_to` excludes the draft's `base_ref`. The metadata now exists; the
  guard is the next consumer (seq-003 §10). Likely the highest-leverage next
  piece.
- **`visibility`** is recorded but **inert** (design §7) — no redaction;
  enforcement deferred to a future service gateway.
- **`data_structure_change`** (concept §6.4) has no CLI setter yet (defaults
  `None`); **cross-release lifecycle** (`first_shipped_in`) is not tracked
  (entries seal `status: active`, `first_shipped_in: null`). Ad-hoc lifecycle
  facets (e.g. `retire-when`) can be recorded today in the seq-003 annotations
  attribute bag; typed graduation with its own checks is deferred (seq-003 §10).
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
