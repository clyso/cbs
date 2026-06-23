# CRT v2 — Plan M3: deterministic SBOM + notes + materialize (artifacts)

> **Status:** Plan (approved). Implements **M3** of the design
> [`../design/001-20260620T1318-v2-mvp.md`](../design/001-20260620T1318-v2-mvp.md)
> (concept [`../000-concept.md`](../000-concept.md)). Part 03 of the multi-part
> plan sharing seq `001` (M1 = part 01, M2 = part 02). Commit boundaries follow
> the `git-commits` skill (capability per commit, ~400–800 LOC soft, lockfiles
> excluded). **No code lands before this breakdown is approved.**

## Scope

After M2 an operator can author, seal, and verify a signed release. M3 turns the
sealed manifest into its two **deterministic projections** and makes them
observable:

- a **CycloneDX SBOM** (`sbom.cdx.json`) — a pure function of the manifest
  (design §7.1);
- **release notes** (`RELEASE-NOTES.md`) — a `minijinja` render pinned by the
  sealed `RenderSpec` (template-by-digest + minijinja version + branding
  snapshot) (design §7.2);

and ships the artifact-emitting half of `release materialize` plus
`release notes` (design §13: "§7.1 deterministic SBOM + §7.2 notes +
`release materialize` (artifacts) + `release notes`").

It also discharges the debt M2 left: `RenderSpec.minijinja_version` was sealed
as a **provisional constant `"2.5.0"`** with `minijinja` not yet linked. M3
links `minijinja` (resolves to **2.21.0**), pins that version, and validates the
default template renders.

**Out of M3** (M4): git materialization (linear `release/<name>` branch +
annotated tag), the signed `000-RELEASE/` bundle, `source_tree_digest`,
`crt verify --tree`, verify **leg 3** (git anchoring), and the **activation** of
verify **leg 4** (artifact faithfulness). Design §11 gates leg 4 on "(ref
exists)" — its byte-compare needs the in-tree committed copies a materialized
ref carries, so leg 4 cannot run until M4. M3 builds the re-derivation engine
leg 4 will reuse; `verify` keeps reporting legs 3–4 `skipped`.

## Reflected design invariants & decisions

- **Both engines live in `crt-core`** (pure, no IO/tokio). Design §7.2 renders
  notes in `crt-core`; the SBOM is a pure function of the manifest. They take
  in-memory inputs (manifest + template bytes); the `crt` binary owns the store
  IO (fetch template by digest, write artifact files).
- **`minijinja_version` is owned by `crt-core`** as `RENDER_MINIJINJA_VERSION`
  (moved out of `crt/src/release.rs`), mirroring how `SCHEMA_VERSION` is the
  single source of truth that seal stamps and verify checks. `crt-core` links
  `minijinja`, so it is the authority on the linked version.
- **Exact-semver pin.** `RENDER_MINIJINJA_VERSION = "2.21.0"` is the exact
  linked version. Design §7.2 says verify "errors on mismatch … rather than
  silently re-rendering" — exact-match semantics, and leg 4's byte-faithfulness
  needs the exact renderer. **Caveat:** a future `minijinja` bump in the binary
  will make re-render/verify error on already-sealed releases — that is the
  intended "use the matching tool build" signal, not a bug. Safe to set now (no
  production release sealed); a signed-bytes contract thereafter. `minijinja` is
  pinned in `crt-core/Cargo.toml` next to the constant so a bump touches both.
- **SBOM is hand-rolled serde structs**, not a CycloneDX crate: §7.1 forbids a
  random `serialNumber`/wall-clock `timestamp`, which off-the-shelf crates
  inject. `Vec`/`BTreeMap` and fixed field order give full determinism control
  and keep `crt-core` dependency-light. CycloneDX 1.6 shape; validated against a
  CycloneDX validator (§7.1/§14).
- **Golden test is undisturbed.** The 2.1 `crt-core` golden fixture already pins
  `minijinja_version: "2.21.0"` and a synthetic `template_digest`, decoupled
  from the seal constant; updating the constant or the template content does not
  shift it. (The "gate-accepted to shift" allowance, M2 plan, was insurance — it
  is not needed.)
- **`materialize` artifact destination:** a local `--out <dir>` (default the
  current directory), creating it if absent. M4 extends the same command with
  git ref/tag + the signed `000-RELEASE/` bundle.

## Commits

### 3.1 — `crt: render release notes from the sealed manifest`

**After this:** `crt release notes <name>` re-renders the canonical notes from a
sealed release's pinned `RenderSpec`.

- `crt-core`: link `minijinja` (default features — `builtins` for `groupby` /
  `title`, `serde` for `Value::from_serialize`; pure-Rust, no C/tokio). New
  `notes` module:
  `render_notes(manifest: &Manifest, template: &str) -> Result<String, CrtCoreError>`
  (renders from `manifest.branding` + entries grouped by `category`;
  `public_summary` only, **never** `justification.internal`);
  `RENDER_MINIJINJA_VERSION` constant; `check_render_version(&RenderSpec)`
  (errors on mismatch). New `CrtCoreError` variants (`Render`,
  `RenderVersionMismatch`).
- Validate/adapt `crt/assets/default-release-notes.md.j2` to minijinja's
  `groupby`-as-filter semantics (re-express group access; pin sort stability);
  the template content may change (does not affect the golden).
- `crt`: replace the local `RENDER_MINIJINJA_VERSION` with
  `crt_core::RENDER_MINIJINJA_VERSION` in `seal_release`;
  `render_sealed_notes(store, cfg, name)` (sealed-only; version-gate; fetch
  template by `render.template_digest`; render); `ReleaseCmd::Notes { name }`.
- **Tests:** notes contain `public_summary`, never `internal`; render from the
  sealed branding snapshot; deterministic re-render; version-mismatch errors;
  the default template renders under linked minijinja.
- **Smell test:** one capability (re-render sealed notes); revertable. **~450
  LOC.**

### 3.2 — `crt: derive a deterministic CycloneDX SBOM and emit release artifacts`

**After this:** `crt release materialize <name> --out <dir>` writes
`sbom.cdx.json` + `RELEASE-NOTES.md`.

- `crt-core`: new `sbom` module — minimal CycloneDX 1.6 serde structs;
  `sbom(manifest: &Manifest) -> Result<String, CrtCoreError>`. Ceph = one
  `component`; each entry under `pedigree.patches[]`
  (`{ type: "backport", … }`). `metadata.timestamp` from `release.created`;
  `serialNumber` derived from the manifest digest — never random/wall-clock.
  Deterministic by fixed field order + sorted collections.
- `crt`: `ReleaseCmd::Materialize { name, out }` — load the sealed release,
  re-derive the SBOM and re-render notes (reusing 3.1), write both files to
  `--out`. Git ref/tag/`000-RELEASE/` bundle **deferred to M4**.
- **Tests:** SBOM byte-determinism (same manifest → identical bytes); golden
  SBOM for a sample manifest; CycloneDX-shape parse; `materialize` writes both
  files; emitted notes == `release notes` output.
- **Smell test:** one capability (deterministic SBOM + artifact emission);
  revertable. **~500 LOC.**

## Decisions, recorded as landed

> Filled in as each commit lands (mirrors the M2 plan's per-commit blocks).

- **3.1** — _pending._
- **3.2** — _pending._

## Progress

| Commit | Subject                                                  | Status |
| ------ | -------------------------------------------------------- | ------ |
| 3.1    | render release notes from the sealed manifest            | ☐ todo |
| 3.2    | derive a deterministic CycloneDX SBOM and emit artifacts | ☐ todo |
