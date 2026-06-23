# CLAUDE.md — cbscore-rs

Guidance for working on the **Rust port of `cbscore`** (the CES Build System
core build library). This directory holds the design corpus; see `README.md` for
the overview, design index, and the operator config-format reference. The crates
(`cbscore-types`, `cbscore`, `cbsbuild`) become members of the `cbsd-rs/`
workspace, so `cbsd-rs/CLAUDE.md` governs build/test/commit mechanics; this file
adds what is specific to the port.

**Read `design/001` first** — it is the spine (crate layout, the two blocker
resolutions, the schema policy, the capability commit map, and the correctness
invariants). Every other design names the subsystem it owns as the single source
of truth.

## Designs are authoritative

- The `design/` documents are the contract. **If code and a design disagree, fix
  the code** (or, if the design is genuinely wrong, amend the design with a new
  review — do not let them drift).
- Each design was adversarially reviewed against the Python source (`reviews/`);
  the reviews capture the verified behavior, the deliberate divergences, and the
  confidence scores.
- Designs are snapshots; do not renumber or retro-edit older designs to match
  later ones. New work gets a new doc per the `seq-docs-convention` /
  `cbsd-rs-docs` skill.
- Markdown here wraps at 79 chars — format with `prettier --write <file>` (see
  `cbsd-rs/docs/CLAUDE.md`); never hand-wrap.

## Settled decisions (do not relitigate without cause)

1. Reimplement fresh; the reference corpus is consulted, not merged.
2. In-process worker integration; **panic isolation at the tokio task boundary**
   via `JoinError::is_panic()` — **not** `catch_unwind` (it cannot span
   `.await`). Keep `panic = "unwind"` (pin it in the workspace profile).
   Cancellation via an explicit `CancellationToken` → runner `podman stop` — no
   async `Drop`.
3. `cbsbuild` is built **static-musl** (`x86_64-unknown-linux-musl`), shipped
   into the worker image, and mounted by explicit path — never "self".
4. Pragmatic schema policy: additive backward-compatible changes do **not**
   bump; only cbscore-owned formats carry `schema-version`; the build report
   keeps `report_version` (convergence is roadmapped).
5. `config init` is dropped; config is hand-authored (README documents formats).
6. Git secrets carry an explicit `type: ssh | token | https` (the one
   operator-visible format change).
7. UUIDv7 auto-versions are in from M1.

## Correctness invariants (from 001 — test each)

1. **Wire round-trip stability** — serialize → parse → equal for every format.
2. **CLI parity** — 010's per-subcommand flag table is authoritative; only
   `--cbscore-path`, `-e/--cbs-entrypoint`, and the whole `config` group are
   intentional drops.
3. **On-disk layout parity** — version store and scratch layout match Python.
4. **Secret redaction** — `SecureArg`; a secret's `Debug`/`Display` emits the
   redacted form (compiler-enforced: the trait has no `Debug` supertrait).
5. **Build-report round-trip** — written in-container to the scratch mount, read
   on the host **before** the rc check (partial-report-on-failure), then
   unlinked.
6. **Failure isolation** — the in-process build runs in a spawned task; a panic
   is mapped to the build-failure path via `JoinError::is_panic()`; the worker
   pins `panic = "unwind"`.
7. **Binary portability** — the static-musl `cbsbuild` runs as PID 1 in the
   oldest supported `desc.distro`.
8. **Vault auth order** — AppRole → userpass → token; KV v2 mount `ces-kv`.
9. **S3 addressing** — explicit creds as a static provider; `endpoint_url` from
   the secrets hostname; `force_path_style(true)` (MinIO/RGW).
10. **Documented hand-authored config** — on-disk output matches the formats 004
    defines and the README renders.

## Fix, don't reproduce (verified Python bugs)

The port **fixes** these with clear contracts (see `README.md` for the full list
and the owning designs): `versions list` arity (006); `get_image_desc`
raw-string filter (006); `--force` no-op on existing release (007); unchecked
`buildah` push rc (008); `can_sign` `ValueError` leak (008); partial-report
discard on failure (009); temp-secrets plaintext leak (009); `cmd_build` dead
secrets write (010). Faithfully-reproduced quirks pending a decision are in
`ROADMAP.md` — reproduce those, do not "fix" them mid-port.

## Implementation order

Build by **capability** (working increments), not by layer — the layer-by-layer
split is the anti-pattern the original review flagged. Follow the capability
commit map in 001 (M0 `C0` bootstrap → M1 `C1` versions create → M2 `C2`–`C6`
build, decomposed by increment → M3 `C7` versions list → M4 `C8` worker cutover
→ M5 `C9` deferred). Each commit must deliver something testable; present the
commit breakdown for approval before implementing a phase, and get an
independent `phase-review` after each phase.
