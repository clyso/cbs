# Plan Review: 013 — Extended Version Info

**Plan:**
`plans/013-20260322T1504-extended-version-info.md`

**Design:**
`design/013-20260322T1210-extended-version-info.md`
(v2, approved)

---

## Summary

The plan faithfully tracks the approved v2 design across
5 commits with correct dependency ordering. Commit sizing
is appropriate. One concern about how the `VERSION` const
in `main.rs` reaches the `health()` function in `app.rs`.

**Verdict: Approve with conditions.**

---

## Design Fidelity

| Design requirement | Plan |
|---|---|
| `build.rs` reads `.git-version` from workspace root | ✓ C2 |
| `CARGO_MANIFEST_DIR.parent()` for workspace path | ✓ (in design) |
| `CBS_BUILD_META` env var | ✓ C2 |
| `VERSION` const in all 3 binaries | ✓ C2 |
| `rerun-if-changed` on `.git-version` | ✓ (in design) |
| `cbsd-server --version` | ✓ C2 |
| `cbsd-worker --version` | ✓ C2 |
| `cbc --version` | ✓ C2 |
| Health endpoint returns version | ✓ C2 |
| `Hello` gains `version: Option<String>` | ✓ C3 |
| `serde(default)` for backwards compat | ✓ C3 |
| Worker sends `VERSION` in Hello | ✓ C3 |
| Server logs version at connect (INFO) | ✓ C3 |
| Server warns on version skew (WARN) | ✓ C3 |
| `WorkerState::Connected` gains `version` | ✓ C3 |
| `WorkerInfoResponse` gains `version` | ✓ C3 |
| `cbc worker list` displays version | ✓ C3 |
| `container/build-cbsd-rs.sh` new script | ✓ C4 |
| `ContainerFile.cbsd-rs` ARG + RUN echo | ✓ C4 |
| Remove compose prod profiles | ✓ C4 |
| README documentation | ✓ C4 |

---

## Commit Breakdown Assessment

### Commit 1 — docs only

Documentation checkpoint. ✓

### Commit 2 — build.rs + VERSION + health (~200 lines)

All 3 `build.rs` files + 3 `main.rs` changes + `app.rs`
health endpoint. Within the 200-400 range. All tightly
coupled — the `build.rs` produces `CBS_BUILD_META`, the
`main.rs` consumes it, and the health endpoint exposes
it. ✓

### Commit 3 — Hello version + workers API (~100 lines)

Proto change + worker handler + server handler + liveness


+ routes + cbc display. Below 200 but independently
meaningful — after this commit, operators can see worker
versions. ✓

### Commit 4 — container + compose (~150 lines)

Build script + Containerfile + compose cleanup + README.
Independent of Commit 3 (no Rust code changes). Below
200 but meaningful — it enables the `.git-version` file
that Commit 2's `build.rs` reads. ✓

### Commit 5 — docs only

Implementation review checkpoint. ✓

---

## Concern

### C1 — `VERSION` in `main.rs` vs `health()` in `app.rs`

The plan says `VERSION` is defined in `main.rs` and the
health endpoint in `app.rs` returns it. `app.rs` is a
module (`pub mod app`) declared in `main.rs`. It can
access `crate::VERSION` — but the plan doesn't specify
this plumbing.


Alternatives:

1. Define `VERSION` in `main.rs` at crate scope, access
   as `crate::VERSION` from `app.rs`. Simple, no state.
2. Put `VERSION` in `app.rs` directly. But then `main.rs`
   needs to import it for `#[command(version = ...)]`.
3. Pass version through `AppState`. Over-engineered for
   a const.

Option 1 is the correct approach and works with zero
overhead. The plan should note that `health()` accesses
`crate::VERSION` (or whatever module path is used).

Not a blocker — the implementation will naturally
discover the right approach. Just noting the implicit
assumption.

---

+# Minor Notes

+ **Dependency ordering is correct.** Commit 2 defines
  `VERSION` → Commit 3 uses it in the Hello message.
  Commit 4 creates the `.git-version` mechanism that
  Commit 2's `build.rs` reads — but Commit 2 compiles
+ without it (defaults to `unknown`). ✓

+ **The server's `#[command]` attribute currently lacks
  `version`.** The plan says all 3 binaries get
  `#[command(version = VERSION)]`. The server's current
  attribute is `#[command(name = "cbsd-server", about)]`
+ — adding `version = VERSION` is correct.

+ **Pattern matches on `WorkerState::Connected { .. }`
  use rest patterns.** Adding `version: Option<String>`
+ to the struct won't break existing match arms. ✓

+ **`cbc` `WorkerInfo` uses `serde::Deserialize`.**
  Adding `version: Option<String>` with `serde(default)`
  is safe — the field deserializes as `None` from older
+ servers that don't send it. ✓

+ **Compose prod profile removal is a user-facing
  change.** The README update in Commit 4 should
  document the migration path for operators currently
  using `--profile prod`.

---

## No Blockers Found

The plan is a faithful, well-structured translation of
the approved design into 5 commits with correct ordering
and appropriate granularity.
