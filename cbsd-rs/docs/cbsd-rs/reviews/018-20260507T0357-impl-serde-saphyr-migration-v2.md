# Review 018-v2 — Implementation: `serde_yml` → `serde-saphyr` migration

- **Component:** `cbsd-rs` (`cbsd-server` + `cbsd-worker`)
- **Design:**
  [018 — serde-saphyr migration](../design/018-20260506T1015-serde-saphyr-migration.md)
- **Plan:**
  [018 — serde-saphyr migration plan](../plans/018-20260506T1213-serde-saphyr-migration.md)
- **Plan review:**
  [018-v1 plan review](./018-20260506T1259-plan-serde-saphyr-migration-v1.md)
- **Prior impl review:**
  [018-v1 impl review](./018-20260506T2254-impl-serde-saphyr-migration-v1.md)
- **Commits in scope:** `1f5fc93`, `d608516`
- **Reviewer:** Claude Sonnet 4.6
- **Review date:** 2026-05-07
- **Status:** Approved

## Executive summary

This v2 review covers the amended tip of `wip/cbsd-rs-saphyre` after commit
`d608516` (an amend of `b477f5e` from the v1 review). The sole open condition
from v1 — the stale plan progress table (D10, −5) — is resolved: all 8 progress
rows and the `Status:` header are now `Done`. No new defects were found. All
toolchain gates pass, five rich-error cases confirm the formatting contract, and
all three real config files parse without error. One factual correction to v1's
lockfile count is noted: `zmij` was already present before the commit series;
the actual new-package count is 9, not 10. The implementation is clean,
complete, and ready to merge.

## 1. Scope

Commits reviewed:

| SHA       | Subject                                                        |
| --------- | -------------------------------------------------------------- |
| `1f5fc93` | `cbsd-rs/docs: capture serde-saphyr migration design and plan` |
| `d608516` | `cbsd-rs: drop serde_yml fork for pure-Rust serde-saphyr`      |

`d608516` is an amend of `b477f5e` reviewed in v1. The delta from `b477f5e` to
`d608516` is confined to the plan file: progress table rows updated from
`Pending` to `Done`, `Status:` header updated from `Pending` to `Done`.

Files inspected:

- `cbsd-rs/cbsd-server/Cargo.toml`
- `cbsd-rs/cbsd-worker/Cargo.toml`
- `cbsd-rs/Cargo.toml` (workspace root — verified not modified)
- `cbsd-rs/cbsd-server/src/config.rs` (lines 338–346)
- `cbsd-rs/cbsd-server/src/components/mod.rs` (lines 51–60)
- `cbsd-rs/cbsd-worker/src/config.rs` (full file)
- `cbsd-rs/cbsd-worker/src/main.rs` (config consumption site)
- `cbsd-rs/Cargo.lock` (full diff against parent of `1f5fc93`)
- `cbsd-rs/docs/cbsd-rs/plans/018-20260506T1213-serde-saphyr-migration.md`
  (post-amend state)

Authoritative YAML files parsed:

- `cbsd-rs/systemd/templates/config/server.yaml.in`
- `cbsd-rs/systemd/templates/config/worker.yaml.in`
- `components/ceph/cbs.component.yaml`

## 2. Method

Commands run (all from worktree root
`/mnt/pci5-dev/clyso/_worktrees/cbs/wip--cbsd-rs-saphyre/`):

```
cargo fmt --manifest-path cbsd-rs/Cargo.toml --all -- --check
cargo clippy --manifest-path cbsd-rs/Cargo.toml --workspace \
    --all-targets -- -D warnings
SQLX_OFFLINE=true cargo check --manifest-path cbsd-rs/Cargo.toml \
    --workspace
SQLX_OFFLINE=true cargo test --manifest-path cbsd-rs/Cargo.toml \
    --workspace
```

Rich-error sanity checks via the compiled `cbsd-worker` binary with `CBSD_DEV=1`
and five distinct bad configs (see §5.2).

Real-config-file parse verified by running `cbsd-worker` against
`worker.yaml.in` (with placeholder token value) in dev mode; parse succeeded,
validation correctly rejected the placeholder base64 token.

Server config verified by running `cbsd-server` in dev mode against a copy of
`server.yaml.in` with `dev.enabled: true`; server reached component loading and
logged "loaded 1 component(s)".

Lockfile delta audited with
`git show d608516 -- cbsd-rs/Cargo.lock | grep '^[+-]name'` and cross-referenced
against the parent commit's lockfile to establish the pre-series baseline.

## 3. Findings

### 3.1 D10 from v1 — plan progress table (resolved)

**Status:** Resolved.

The v1 review deducted −5 for D10 because all 8 progress rows remained `Pending`
and the `Status:` header read `Pending` after `b477f5e`. The amend (`d608516`)
updates every row in the progress table to `Done` and the header to `Done`.
Confirmed by reading the post-amend plan file; the deduction is lifted in v2.

### 3.2 v1 lockfile count: zmij correction (factual note, no deduction)

The v1 executive summary and §6.1 stated "10 added packages including `zmij`."
Verified by inspecting `git show 2ed9f0c:cbsd-rs/Cargo.lock` (the commit
immediately before the series): `zmij` is already present. The lock diff shows a
new `dependencies` reference from `serde-saphyr` to the pre-existing `zmij`
package entry, not a new package. The correct count of new packages introduced
by `d608516` is **9**: `ahash`, `annotate-snippets`, `arraydeque`,
`encoding_rs`, `encoding_rs_io`, `granit-parser`, `nohash-hasher`,
`serde-saphyr`, `unicode-width`.

This is a factual correction to v1's narrative, not an implementation defect. No
deduction applied.

### 3.3 User-accepted items carried forward (acknowledged)

The following items were explicitly declined by the user before implementation
and are not re-raised as defects in v2:

- **No automated rich-error test** — v1 §3.3; manual sanity check performed in
  §5.2 below.
- **No `#[non_exhaustive]` doc comment on `ConfigError::Parse`** — v1 §3.4;
  behaviour accessible only through `Display`/`source`.

## 4. Plan adherence (§1.1–§1.5)

All sections verified against the post-amend plan file and the source code.
Findings are identical to v1 §4 except where explicitly noted below; this
section records the re-verification for completeness.

### §1.1 — Cargo.toml edits (per-crate)

Verified. `cbsd-server/Cargo.toml` and `cbsd-worker/Cargo.toml` carry
`serde-saphyr = "0.0.26"` in place of `serde_yml = "0.0.12"`. The workspace root
`cbsd-rs/Cargo.toml` is unmodified; `serde-saphyr` is not in
`[workspace.dependencies]`, consistent with the per-crate resolution from plan
§"Decisions resolved".

Dep ordering: `serde-saphyr` occupies the same slot `serde_yml` held (between
`serde_json` and `sha2`). `serde-` (ASCII 0x2D) sorts before `serde_` (ASCII
0x5F) in strict ASCII order, so the entry is not in strict alpha order relative
to `serde_json`. This mirrors the pre-existing `serde_yml` placement and is
consistent with project style. Not flagged.

### §1.2 — `cbsd-server/src/components/mod.rs`

Verified at line 54. `serde_yml::from_str` → `serde_saphyr::from_str`. Format
string `"failed to parse {}: {e}"` → `"failed to parse '{}':\n{e}"`. Matches
plan diff exactly.

### §1.3 — `cbsd-server/src/config.rs`

Verified at line 342. Parser swapped. Panic format string updated from
`"failed to parse config file {}: {e}"` to
`"failed to parse config file '{}':\n{e}"`. Matches plan diff exactly.

Line 341 (adjacent `read` panic — unquoted, no newline) intentionally untouched
per §1.5. Confirmed.

### §1.4 — `cbsd-worker/src/config.rs`

All three coupled edits verified:

1. `ConfigError::Parse` variant: `Parse(serde_yml::Error)` →
   `Parse(PathBuf, Box<serde_saphyr::Error>)`. The `Box<>` was added relative to
   the original design proposal to avoid `clippy::result_large_err`; the plan
   documents this with rationale in the post-amend state. Clippy is clean.

2. Call site (lines 136–137):
   `.map_err(|e| ConfigError::Parse(path.to_path_buf(), Box::new(e)))?` `path`
   is in scope. `PathBuf` is imported at line 13. Matches plan.

3. `Display` arm for `Parse`: emits
   `"failed to parse config file '{}':\n{err}"`. `source` arm for `Parse`:
   returns `Some(err)` — `Box<T>` auto-derefs to `T` for the
   `dyn std::error::Error` coercion. No external constructors of
   `ConfigError::Parse` outside `config.rs:137`; confirmed by grep.

### §1.5 — Read/parse formatting asymmetry is intentional

Read/parse formatting asymmetry documented in §1.5 of the plan. Adjacent panics
in `config.rs` confirmed untouched. `ConfigError::Read` Display arm confirmed
unmodified. No defect.

### Plan deviation: `Box<serde_saphyr::Error>`

Plan updated in `d608516` with the `Box::new(e)` form and a rationale paragraph.
Plan and code are coherent. Progress table reflects this step as `Done`.

## 5. Verification results

### 5.1 Toolchain checks

All run from worktree root with no failures:

| Command                                                 | Exit | Output              |
| ------------------------------------------------------- | ---- | ------------------- |
| `cargo fmt --all -- --check`                            | 0    | none                |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0    | none                |
| `SQLX_OFFLINE=true cargo check --workspace`             | 0    | none                |
| `SQLX_OFFLINE=true cargo test --workspace`              | 0    | 143 tests, 0 failed |

Test distribution: `cbc` (6), `cbsd-proto` (21), `cbsd-server` (104),
`cbsd-worker` (12).

### 5.2 Rich-error sanity check (5 cases)

All cases run against the compiled `cbsd-worker` binary with `CBSD_DEV=1` from
the worktree root. Each case confirms: path-qualified header on line 1, `\n`
separator, rustc-style `-->` pointer on its own line.

**Case 1 — Type mismatch (sequence for string field):**

Input: `server-url: [bad, value]`

```
error: failed to parse config file '/tmp/cbsd-worker-bad-type.yaml':
error: line 1 column 13: unexpected event: expected string scalar
 --> <input>:1:13
  |
1 | server-url: [bad, value]
  |             ^ unexpected event: expected string scalar
```

Path-qualified header, newline, caret at column 13. Correct.

**Case 2 — Missing required field:**

Input: omit `server-url`.

```
error: failed to parse config file '/tmp/cbsd-worker-missing-field.yaml':
error: line 2 column 1: missing field `server-url`
 --> <input>:2:1
  |
1 | api-key: "cbsk_0000...
2 | arch: "x86_64"
  | ^ missing field `server-url`
```

Kebab-case field name preserved. Correct.

**Case 3 — Wrong scalar type for numeric field:**

Input: `build-timeout-secs: [not, a, number]`

Parse error produced with caret pointing at `[`. Correct.

**Case 4 — Duplicate key:**

Input: `server-url` repeated on lines 1 and 2.

```
error: failed to parse config file '/tmp/cbsd-worker-dup-key.yaml':
error: line 2 column 1: duplicate mapping key: server-url,
  set DuplicateKeyPolicy in Options if acceptable
 --> <input>:2:1
```

`serde-saphyr` rejects duplicate keys by default with a helpful hint. Stricter
than `serde_yml` (which silently took the last value). Upside.

**Case 5 — Unknown field (no `deny_unknown_fields`):**

Input: added `unknown-mystery-field: "x"`.

Worker proceeded past parse to attempt WebSocket connection (connection refused
at `ws://localhost:8080`). Unknown field silently ignored, matching design audit
item 4. No spurious error.

In all five cases the formatting contract is correct: path-qualified header,
newline before the snippet, `-->` pointer on its own line.

### 5.3 Real config file parse

**`worker.yaml.in`:** Parsed via
`cbsd-worker --config /tmp/cbsd-worker-template-dev.yaml` (dev mode, no
log-file). Result: parse succeeded; validation rejected the placeholder base64
token (`invalid worker token base64: Invalid symbol 46, offset 15`). Error is at
validation, not parse — YAML structure is accepted.

**`server.yaml.in`:** Parsed via `cbsd-server` in dev mode with
`dev.enabled: true`. Server reached component loading and logged "loaded 1
component(s)". Parse succeeded.

**`components/ceph/cbs.component.yaml`:** Confirmed loaded successfully via
server startup log. Parse succeeded.

All three real config files parse without error under `serde-saphyr`.

## 6. Comparison with v1

| Area                              | v1 finding            | v2 status                          |
| --------------------------------- | --------------------- | ---------------------------------- |
| D10: plan progress table          | −5 (all rows Pending) | Resolved — all rows `Done`         |
| D10: Status header                | stale (`Pending`)     | Resolved — header `Done`           |
| Lockfile count (zmij)             | "10 added incl. zmij" | Corrected to 9 (zmij pre-existing) |
| Commit subject deviation          | nit (cosmetic)        | Unchanged — no action required     |
| User-accepted: no auto test       | acknowledged          | Carried forward — no deduction     |
| User-accepted: non_exhaustive doc | acknowledged          | Carried forward — no deduction     |
| All other findings                | clean                 | Re-verified clean                  |

## 7. Forward-looking risks

The risks documented in v1 §6 are unchanged. They are summarised here for
completeness; no new risks were identified.

### 7.1 Supply-chain delta

2 packages removed (`serde_yml`, `libyml`), 9 new packages added (corrected from
v1's count of 10). Notable entries:

- `granit-parser 0.0.2` — YAML tokenizer at the bottom of the saphyr stack.
  `0.0.2` is earlier-stage than `serde-saphyr 0.0.26`. Small, pure-Rust, depends
  only on `arraydeque` and `smallvec`. Warrants inclusion in the project's
  dependency risk register.
- `encoding_rs 0.8.35` — well-established, contains `unsafe` for SIMD but is
  pure Rust with no C FFI. Used for YAML stream encoding detection.
- `ahash 0.8.12` — active, widely used in the Rust ecosystem.

Net picture is healthier than `serde_yml`'s C-derived tree.

### 7.2 Zerover upgrade story

`serde-saphyr = "0.0.26"` and `granit-parser 0.0.2` are both zerover. Cargo
treats `"0.0.x"` as equivalent to `=0.0.x` — only that exact version satisfies
the constraint. Upgrading requires deliberate bumps in both
`cbsd-server/Cargo.toml` and `cbsd-worker/Cargo.toml`. Treat each bump as
requiring re-verification of rich-error output format (which may change between
pre-1.0 releases).

### 7.3 `Box<serde_saphyr::Error>` ergonomics

`ConfigError::Parse(PathBuf, Box<serde_saphyr::Error>)` is constructed at
exactly one site and consumed only via `Display` and
`std::error::Error::source`. `Box<T>` auto-derefs cleanly in both contexts.
Future code that pattern-matches `ConfigError` will encounter a `Box` in the
`Parse` arm, but because `serde_saphyr::Error` is `#[non_exhaustive]`, the only
safe inspection path is through `Display` or `source` regardless. No ergonomic
cost in current or likely future usage.

## 8. Confidence score

| Item                              | Points  | Description                              |
| --------------------------------- | ------- | ---------------------------------------- |
| Starting score                    | 100     |                                          |
| D10 from v1 (plan progress table) | 0       | Resolved in `d608516` — deduction lifted |
| **Total**                         | **100** |                                          |

**Interpretation:** 100/100 — ready to merge. The single deduction from v1 is
resolved. All toolchain gates pass, rich-error output is correct across five
diverse error cases, and all three real config templates parse successfully. No
new defects found.

**Recommendation: Approve.** No conditions. The implementation is complete as
specified.
