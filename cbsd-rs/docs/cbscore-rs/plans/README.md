# cbscore-rs Implementation Plans

## Overview

Phased, **capability-sliced** implementation plan for the Rust port of the
Python `cbscore` build library into three new `cbsd-rs/` workspace crates
(`cbscore-types`, `cbscore`, `cbsbuild`). Work is sliced by **capability — what
an operator or the worker can do — never by layer**; foundational code (types,
the subprocess primitive, tool wrappers, the S3 client, buildah) lands in the
commit of its **first consumer**, never as a standalone layer.

**Design documents (authoritative):** `cbsd-rs/docs/cbscore-rs/design/` — read
`design/001` (the spine) first; it owns the capability commit map these plans
are derived from. **If code and a design disagree, fix the code.**

All plans share the spine's sequence `001` (per `design/001`); the 2-digit
sub-number orders the milestones.

## Implementation Status

| Phase                                         | Milestone | Capability                                                 | Commits | Status  |
| --------------------------------------------- | --------- | ---------------------------------------------------------- | ------- | ------- |
| [M0](001-20260623T1725-01-bootstrap.md)       | Bootstrap | Static `cbsbuild` links musl + CI proof (C0)               | 2       | Done    |
| [M1](001-20260623T1725-02-versions-create.md) | Versions  | `versions create [VERSION]` writes a descriptor (C1)       | 3       | Done    |
| [M2](001-20260623T1725-03-build.md)           | Build     | `build <desc>`: container → RPMs → sign+S3 → image (C2–C6) | 8       | Pending |
| [M3](001-20260623T1725-04-versions-list.md)   | List      | `versions list --from` lists releases (C7, fixed)          | 1       | Pending |
| [M4](001-20260623T1725-05-worker.md)          | Worker    | `cbsd-worker` runs builds in-process (C8)                  | 2       | Pending |

**Total:** 16 commits across 5 milestones (M1 split 2→3 at implementation — see
its plan). The per-commit counts above are the recommended breakdown (the v1
plan-review's findings — the M2/C4 split, the C5 split decision, the C0 CI gate,
the M4 image re-base — are folded into the plans); each milestone's exact
boundaries are confirmed at its commit-breakdown approval. See each plan's
"Notes for the plan-review".

## Dependency Graph

```
M0/C0 ─→ M1/C1 ─→ M2 ( C2a ─→ C2b ─→ C3 ─→ C4a ─→ C4b ─→ C5a ─→ C5b ─→ C6 ) ─→ M3/C7 ─→ M4/C8
```

Linear by capability: each milestone reuses the foundation the previous ones
landed. M3 (`versions list`) is sequenced after M2 specifically to reuse the
config/secrets/S3 infrastructure C2/C4a land (it adds only `s3_list`). M4 (the
worker cutover) depends on the host `runner::run` (M2/C2) and the descriptor
helpers (M1/C1).

## Deferred (post-port)

- **M5 / C9 — configurable version-descriptor location.** Designed now in
  `design/006` (the `Config.paths.versions` field + `--versions-dir` flag +
  `descriptor_path` helper, precedence flag > config > `<git-root>/_versions`),
  **built later**. M1's `versions create` uses the hardcoded `_versions`
  default. Trigger: when an operator needs a store outside the git checkout.
- **Faithfully-reproduced quirks** (reproduced now, decided later) live in
  `cbsd-rs/docs/cbscore-rs/ROADMAP.md`: the silent component drop when a
  component has no `get_release_rpm.sh` (F2), and `get_release_rpm.sh` running
  with no cwd (F3). The port reproduces these with clear error logs; it does
  **not** "fix" them mid-port.

## Conventions

- **Capability, not layer.** Every commit delivers something testable and ships
  no dead code (foundational code lands with its first consumer). Present each
  milestone's commit breakdown for approval **before** implementing it.
- **Review gates.** Each plan is run through the **`adversarial-review`** skill
  (TYPE = plan) before its implementation begins; each milestone gets an
  independent review after it lands. Reviews are recorded under
  `cbsd-rs/docs/cbscore-rs/reviews/`.
- **Commit mechanics** (per `cbsd-rs/CLAUDE.md`): DCO sign-off (`-s`), no GPG
  signing autonomously, exactly one `Co-authored-by` trailer on autonomous
  commits, Ceph commit-message style; each commit compiles
  (`cargo build --workspace`) and passes `cargo clippy --workspace` +
  `cargo fmt --all --check`.
- **Update this status table** after each commit lands; flip the milestone's
  per-commit progress table in its own plan file too.
- **Markdown wraps at 79 cols** — format with `prettier --write <file>` (see
  `cbsd-rs/docs/CLAUDE.md`); never hand-wrap.
