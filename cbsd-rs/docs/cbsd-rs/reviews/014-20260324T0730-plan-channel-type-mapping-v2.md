# 014 — Plan Review: Channel/Type Mapping (v2)

**Plan:**
`docs/cbsd-rs/plans/014-20260324T0037-channel-type-mapping.md`

**Prior review:**
`014-20260324T0718-plan-channel-type-mapping-v1`

**Verdict:** Ready for implementation

---

## v1 Finding Resolution

| # | Finding | Status |
|---|---------|--------|
| F1 | Commit 3 breaks compilation | Fixed — all 5 consumer files listed; ~60 line estimate; bridging described per file |
| F2 | Resolution logic placement | Fixed — shared `resolve_and_rewrite()` in `channels/mod.rs`; both paths call it |
| F3 | Periodic scope checking | Fixed — explicit: re-check at trigger time; revoked scopes cause failure |
| F4 | Missing `permissions.rs` in commit 4 | Fixed — added with `KNOWN_CAPS` update |
| F5 | `extractors.rs` in commit 5 | Fixed — removed; existing pattern matching suffices |

All v1 findings resolved.

---

## Minor Note

### N1 — `cbsd-proto/src/ws.rs` missing from commit 3

Commit 3 lists `cbsd-proto/src/build.rs` with "update
tests." There is also a test in `cbsd-proto/src/ws.rs`
(line 150) that constructs a `BuildDescriptor` with
`channel: "ces".to_string()` — this needs
`Some(...)` wrapping too.

The implementer would catch this immediately on
`cargo build`, but since the plan lists files
explicitly, `ws.rs` should be included.

---

## Assessment

The plan is complete and internally consistent.
Every commit compiles independently. The dependency
chain is correct. The shared resolution helper avoids
duplication. Scope re-checking for periodic triggers
is explicit. Sizing estimates are realistic, with a
documented split strategy for the largest commit.

Ready for implementation.
