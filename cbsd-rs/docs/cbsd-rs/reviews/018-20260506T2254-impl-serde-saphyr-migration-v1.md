# Review 018-v1 — Implementation: `serde_yml` → `serde-saphyr` migration

- **Component:** `cbsd-rs` (`cbsd-server` + `cbsd-worker`)
- **Design:**
  [018 — serde-saphyr migration](../design/018-20260506T1015-serde-saphyr-migration.md)
- **Plan:**
  [018 — serde-saphyr migration plan](../plans/018-20260506T1213-serde-saphyr-migration.md)
- **Plan review:**
  [018-v1 plan review](./018-20260506T1259-plan-serde-saphyr-migration-v1.md)
- **Implementation commit:** `b477f5e`
- **Reviewer:** Claude Sonnet 4.6
- **Review date:** 2026-05-06
- **Status:** Approved with conditions

## Executive summary

The implementation is mechanically correct, all four source edits match the plan
exactly, and every verification gate passes. The one concrete defect is that the
plan's progress table and status header were not updated to reflect completion
in the implementation commit, despite `cbsd-rs/CLAUDE.md` requiring progress
tables to be updated after each commit lands. Two concerns from the v1 plan
review were explicitly declined by the user (automated rich-error test,
`#[non_exhaustive]` doc comment) and are acknowledged here as user-accepted
risk, not deductions. The supply-chain delta is larger than the "drop a forked C
parser" framing implies — 10 new lockfile entries replace 2 — with
`granit-parser 0.0.2` worth monitoring as the earliest-stage new transitive
dependency.

## 1. Scope

Commits reviewed:

| SHA       | Subject                                                        |
| --------- | -------------------------------------------------------------- |
| `1f5fc93` | `cbsd-rs/docs: capture serde-saphyr migration design and plan` |
| `b477f5e` | `cbsd-rs: drop serde_yml fork for pure-Rust serde-saphyr`      |

Files inspected:

- `cbsd-rs/cbsd-server/Cargo.toml`
- `cbsd-rs/cbsd-worker/Cargo.toml`
- `cbsd-rs/Cargo.toml` (workspace root — verified not modified)
- `cbsd-rs/cbsd-server/src/config.rs` (lines 338–346)
- `cbsd-rs/cbsd-server/src/components/mod.rs` (lines 51–60)
- `cbsd-rs/cbsd-worker/src/config.rs` (full file)
- `cbsd-rs/cbsd-worker/src/main.rs` (config consumption site)
- `cbsd-rs/Cargo.lock` (full diff)
- `cbsd-rs/docs/cbsd-rs/plans/018-20260506T1213-serde-saphyr-migration.md`
  (in-tree copy, post-implementation)
- `cbsd-rs/docs/cbsd-rs/design/018-20260506T1015-serde-saphyr-migration.md`
- `cbsd-rs/docs/cbsd-rs/reviews/018-20260506T1259-plan-serde-saphyr-migration-v1.md`

Authoritative YAML files parsed:

- `cbsd-rs/systemd/templates/config/server.yaml.in`
- `cbsd-rs/systemd/templates/config/worker.yaml.in`
- `components/ceph/cbs.component.yaml`

## 2. Method

Commands run (all from worktree root
`/mnt/pci5-dev/clyso/_worktrees/cbs/wip--cbsd-rs-saphyre/`):

```
cargo fmt --manifest-path cbsd-rs/Cargo.toml --all -- --check
cargo clippy --manifest-path cbsd-rs/Cargo.toml --workspace
    --all-targets -- -D warnings
SQLX_OFFLINE=true cargo check --manifest-path cbsd-rs/Cargo.toml
    --workspace
SQLX_OFFLINE=true cargo test --manifest-path cbsd-rs/Cargo.toml
    --workspace
```

Rich-error sanity checks via the compiled `cbsd-worker` binary with `CBSD_DEV=1`
and five distinct bad configs (see §5).

Real-config-file parse verified by running `cbsd-worker` against
`worker.yaml.in` (with placeholder token value) in dev mode; parse succeeded,
validation correctly rejected the placeholder base64 token.

Cargo.lock diff inspected with `git show b477f5e -- cbsd-rs/Cargo.lock` to
enumerate added/removed packages.

## 3. Findings

### 3.1 Plan progress table not updated (minor)

**Severity:** Minor — convention violation (not a correctness issue)

**Location:**
`cbsd-rs/docs/cbsd-rs/plans/018-20260506T1213-serde-saphyr-migration.md`,
progress table and `Status:` header

The implementation commit (`b477f5e`) modified this plan file to add the
`Box<serde_saphyr::Error>` deviation note — so the file was in scope for the
commit — but the progress table and the `Status:` header were not updated. All 8
rows remain `Pending` and the header still reads `Status: Pending`.

`cbsd-rs/CLAUDE.md` states: "Update plan progress tables after completing each
commit." The commit is complete; the table is stale.

**Recommended fix:** In the next commit (or as a standalone documentation fix),
update all 8 progress rows to `Done` and the header to `Status: Done` (or
`Implemented`).

### 3.2 Commit subject deviates from planned subject (nit)

**Severity:** Nit — cosmetic

**Location:** `b477f5e` subject line vs plan §"Commit message"

Plan specified:

```
cbsd-rs: replace serde_yml with serde-saphyr for YAML parsing
```

Actual subject:

```
cbsd-rs: drop serde_yml fork for pure-Rust serde-saphyr
```

The actual subject is arguably more informative ("fork" and "pure-Rust" carry
meaning the plan's version omits), and commit message text in the plan is a
recommendation not a mandate. No action required; noted for transparency.

### 3.3 User-accepted: no automated rich-error test (acknowledged)

The v1 plan review (Concern 2, score −5) recommended extending the worker config
unit test to assert that invalid YAML produces a `ConfigError::Parse` whose
`Display` contains `\n`. No such test exists in `cbsd-worker/src/config.rs` —
the file has no `#[cfg(test)]` block. The user declined this recommendation
before implementation. Confirmed: no deduction applied; behaviour verified by
manual sanity check (§5).

### 3.4 User-accepted: no `#[non_exhaustive]` doc comment (acknowledged)

The v1 plan review (Concern 3) recommended adding a one-line comment on
`ConfigError::Parse` noting `serde_saphyr::Error` is `#[non_exhaustive]` and is
only accessed through `Display`/`source`. The user declined. No deduction
applied; noted for transparency.

## 4. Plan adherence (§1.1–§1.5)

### §1.1 — Cargo.toml edits (per-crate)

Verified. `cbsd-server/Cargo.toml` and `cbsd-worker/Cargo.toml` each carry
`serde-saphyr = "0.0.26"` in place of `serde_yml = "0.0.12"`. No changes to
`cbsd-rs/Cargo.toml` (workspace root); `serde-saphyr` is not in
`[workspace.dependencies]`, matching the per-crate decision.

Dep ordering: `serde-saphyr` lands between `serde_json` and `sha2` in both files
— consistent with the pre-existing ordering of `serde_yml` at the same position.
Alphabetically `serde-` < `serde_` (ASCII 45 < 95) so placing `serde-saphyr`
after `serde_json` is mildly inconsistent in strict ASCII order, but it mirrors
the exact slot `serde_yml` occupied and is consistent with project style. No
change required.

### §1.2 — `cbsd-server/src/components/mod.rs`

Verified at line 54. Parser swapped `serde_yml::from_str` →
`serde_saphyr::from_str`. Format string updated from `"failed to parse {}: {e}"`
to `"failed to parse '{}':\n{e}"`. Matches plan diff exactly.

### §1.3 — `cbsd-server/src/config.rs`

Verified at line 342. Parser swapped. Panic format string updated from
`"failed to parse config file {}: {e}"` to
`"failed to parse config file '{}':\n{e}"`. Matches plan diff exactly.

The immediately preceding line 341 (`read` panic — unquoted path, no newline) is
intentionally untouched per plan §1.5, which documents this asymmetry as
deliberate. Confirmed; no defect.

### §1.4 — `cbsd-worker/src/config.rs`

All three coupled edits verified:

1. `ConfigError::Parse` variant changed from `Parse(serde_yml::Error)` to
   `Parse(PathBuf, Box<serde_saphyr::Error>)`. The `Box<>` was added relative to
   the design's original proposal, rationale documented in-plan
   (`serde_saphyr::Error` ≥128 bytes, avoids `clippy::result_large_err`).
   Confirmed clippy is clean.

2. Call site at line 136–137:
   `.map_err(|e| ConfigError::Parse(path.to_path_buf(), Box::new(e)))?` Matches
   plan diff. `path` is in scope (`&std::path::Path` parameter to
   `WorkerConfig::load`). `PathBuf` is already imported at the top of the file
   (line 13: `use std::path::PathBuf;`).

3. `Display` and `source` impls updated correctly. `Display` for `Parse` emits
   `"failed to parse config file '{}':\n{err}"`. `source` for `Parse` returns
   `Some(err)` (auto-deref through `Box` transparently implements
   `std::error::Error`). Callers in `main.rs:104` and `main.rs:124` consume
   `ConfigError` only via `eprintln!("error: {err}")` (Display) — no external
   match on `Parse` exists anywhere in the codebase. Box ergonomics are clean.

No other constructors of `ConfigError::Parse` exist outside `config.rs:137`;
confirmed by exhaustive grep.

### §1.5 — Read/parse formatting asymmetry is intentional

Confirmed as a documented design decision in the in-tree plan. Both asymmetries
noted in the v1 plan review (server `load_config` adjacent panics; worker
`ConfigError::Read` vs `ConfigError::Parse` Display) are covered by §1.5's
rationale. No deduction.

### Plan deviation: `Box<serde_saphyr::Error>`

The plan was correctly updated in `b477f5e` to document the `Box<>` deviation
with rationale. The pre-`Box` diff snippet in the plan was replaced with the
`Box::new(e)` form, and a paragraph explaining the size concern was added. Plan
and code are coherent.

## 5. Verification results

### 5.1 Toolchain checks

All run from worktree root with no failures:

| Command                                                 | Exit | Warnings |
| ------------------------------------------------------- | ---- | -------- |
| `cargo fmt --all -- --check`                            | 0    | none     |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0    | none     |
| `SQLX_OFFLINE=true cargo check --workspace`             | 0    | none     |
| `SQLX_OFFLINE=true cargo test --workspace`              | 0    | 0 failed |

Test suite: 143 tests pass across `cbc` (6), `cbsd-proto` (21), `cbsd-server`
(104), `cbsd-worker` (12).

### 5.2 Rich-error sanity check

Five cases exercised against the built `cbsd-worker` binary with `CBSD_DEV=1`:

**Case 1 — Type mismatch (sequence for string field):**

Input: `server-url: [bad, value]`

Output:

```
error: failed to parse config file '/tmp/cbsd-worker-bad-type.yaml':
error: line 1 column 13: unexpected event: expected string scalar
 --> <input>:1:13
  |
1 | server-url: [bad, value]
  |             ^ unexpected event: expected string scalar
2 | api-key: "cbsk_0000…
3 | arch: "x86_64"
  |
```

Path-qualified header, newline, rustc-style caret — correct.

**Case 2 — Unknown field (no `deny_unknown_fields`):**

Input added `unknown-mystery-field: "should be silently ignored"`.

Worker started, proceeded to attempt WebSocket connection (connection refused) —
parse succeeded, unknown field silently ignored. Matches design audit item 4.

**Case 3 — Missing required field:**

Input omitted `server-url`.

Output:

```
error: failed to parse config file '/tmp/cbsd-worker-missing-field.yaml':
error: line 2 column 1: missing field `server-url`
 --> <input>:2:1
  |
1 | api-key: "cbsk_0000…
2 | arch: "x86_64"
  | ^ missing field `server-url`
```

Field name `server-url` matches the kebab-case rename. Correct.

**Case 4 — Indentation error:**

Input had an unexpectedly indented line.

Output:

```
error: failed to parse config file '/tmp/cbsd-worker-indent-err.yaml':
error: line 3 column 3: while parsing a block mapping,
  did not find expected key
 --> <input>:3:3
  |
1 | server-url: "ws://localhost:8080/api/ws/worker"
2 | api-key: "cbsk_0000…
3 |   bad-indent: this is wrong
  |   ^ while parsing a block mapping, did not find expected key
4 | arch: "x86_64"
  |
```

Structural YAML error caught with source location. Correct.

**Case 5 — Duplicate key:**

Input had `server-url` repeated.

Output:

```
error: failed to parse config file '/tmp/cbsd-worker-dup-key.yaml':
error: line 2 column 1: duplicate mapping key: server-url,
  set DuplicateKeyPolicy in Options if acceptable
 --> <input>:2:1
  |
1 | server-url: "ws://localhost:8080/api/ws/worker"
2 | server-url: "ws://localhost:9090/api/ws/worker"
  | ^ duplicate mapping key: server-url, …
3 | api-key: "cbsk_0000…
4 | arch: "x86_64"
  |
```

`serde-saphyr` rejects duplicate keys by default, with a helpful hint. This is
strictly better than `serde_yml`, which silently accepted the last value. Noted
as upside.

In all error cases: path-qualified header on first line, newline separator,
rustc-style `-->` pointer on its own line — formatting is correct and legible.

### 5.3 Real config file parse

`worker.yaml.in` parsed with placeholder values via
`cbsd-worker --config /tmp/cbsd-worker-template-dev.yaml` (dev mode, no log
file); result: config parse succeeded, validation rejected the placeholder
base64 token (`invalid worker token base64: Invalid symbol 46, offset 15`). The
error is at validation, not parse — confirming the YAML structure is accepted by
`serde-saphyr`.

`server.yaml.in` and `components/ceph/cbs.component.yaml`: verified structurally
clean by Python `yaml.safe_load` and manual inspection against every YAML 1.1
divergence class (Norway problem, leading-zero integers, sexagesimals, unquoted
`@:#`, anchors, multi-document streams, duplicate keys) — none present. The
`serde-saphyr` default `Options` accept YAML 1.1 boolean aliases
(`yes`/`no`/`on`/`off`); none appear in these files. Behaviour-preserving.

## 6. Forward-looking risks

### 6.1 Supply-chain delta is wider than the headline suggests

The commit description frames the change as "drop the C-derived parser." The
Cargo.lock diff shows 2 packages removed (`serde_yml`, `libyml`) and 10 added
(`ahash`, `annotate-snippets`, `arraydeque`, `encoding_rs`, `encoding_rs_io`,
`granit-parser`, `nohash-hasher`, `serde-saphyr`, `unicode-width`, `zmij`).
Notable entries:

- `granit-parser 0.0.2` — the YAML tokenizer at the bottom of the saphyr stack.
  Version `0.0.2` is significantly earlier-stage than `serde-saphyr 0.0.26`.
  Neither the design nor the plan audits this dependency. It is small (pure
  Rust, depends only on `arraydeque` and `smallvec`), but its maturity warrants
  a note in the risk register.
- `encoding_rs 0.8.35` — a well-established pure-Rust Unicode encoding crate
  used by saphyr for YAML stream encoding detection. Despite containing `unsafe`
  for SIMD, it is not a C library; no FFI.
- `ahash 0.8.12` — used internally by `serde-saphyr` for map hashing. Active
  project, widely used in the Rust ecosystem.
- All other new entries are straightforward: pure-Rust data structure helpers
  with minimal dep footprints.

The net picture is healthier than `serde_yml`'s C-derived tree, but operators
and security teams should be aware the swap adds 8 new first-depth transitive
dependencies, not 1.

### 6.2 Pre-1.0 bump story for `serde-saphyr`

`serde-saphyr 0.0.26` is zerover. Cargo's semver rules make `"0.0.26"`
equivalent to `=0.0.26` — only that exact version satisfies the constraint.
Upgrading requires a deliberate version bump in both `Cargo.toml` files. This
matches the pattern from `serde_yml = "0.0.12"` and is a strict improvement in
maintenance trajectory. No documented migration guide exists for
`0.0.x → 0.0.(x+1)` bumps; treat each bump as requiring a re-test of the
rich-error output format (which may change between pre-1.0 releases).

`granit-parser 0.0.2` is at an even earlier stage and carries the same zerover
caution.

### 6.3 `Box<serde_saphyr::Error>` ergonomics

`ConfigError::Parse(PathBuf, Box<serde_saphyr::Error>)` is constructed at
exactly one site and consumed only via `Display` and
`std::error::Error::source`. `Box<T>` auto-derefs to `T` in both contexts, so
callers never need `&*err` syntax. `source()` correctly returns `Some(err)` —
the deref coercion through `Box` applies. No ergonomic cost in current usage.
Future code that pattern-matches on `ConfigError` will see a `Box` in the
`Parse` arm, which is slightly less ergonomic to inspect than a bare `E`, but
the only safe way to inspect `serde_saphyr::Error` is through `Display` or
`source` regardless (it is `#[non_exhaustive]`). No action required.

## 7. Confidence score

| Item                                 | Points | Description                                                                                                                          |
| ------------------------------------ | ------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| Starting score                       | 100    |                                                                                                                                      |
| D10: plan progress table not updated | −5     | All 8 rows remain `Pending`; `Status: Pending` header unchanged; `cbsd-rs/CLAUDE.md` requires tables updated after each commit lands |
| **Total**                            | **95** |                                                                                                                                      |

**Interpretation:** 95/100 — ready to merge with the minor note that the
progress table should be updated. The single deduction is a documentation
discipline issue, not a correctness or safety concern. All toolchain gates pass,
rich-error output is correct across five diverse error cases, real config
templates parse successfully, and the `Box<>` deviation is correctly documented
and motivated.

**Recommendation: Proceed as-is.** Fix the progress table as a follow-on
documentation commit (or squash it in if the branch has not been shared). No
rework of the implementation is required.

## Appendix: declined v1 plan review concerns

For traceability, the two concerns from the v1 plan review that were explicitly
declined before implementation:

| Concern                                                     | v1 score hit | Status                                         |
| ----------------------------------------------------------- | ------------ | ---------------------------------------------- |
| D5: no automated test for rich-error snippet rendering      | −5           | Declined; manual sanity check performed (§5.2) |
| D9: `#[non_exhaustive]` doc comment on `ConfigError::Parse` | n/a (green)  | Declined; noted in §3.4                        |

The third concern (read/parse formatting asymmetry) was incorporated into the
plan as §1.5 and is fully addressed.
