# Implementation Review: cbc CLI — All Phases

**Commits reviewed:**
- `f91fd2d` — scaffold with login and whoami (Plan 00+01)
- `bbb360b` — build commands (Plan 02)
- `53d455c` — build log commands (Plan 03)
- `6b31a4c` — periodic build commands (Plan 04)
- `0604d27` — worker admin commands (Plan 05)
- `c217529` — admin role and queue commands (Plan 06 C1)
- `70cf5b8` — admin user and role assignment commands (Plan 06 C2)

**Evaluated against:**
- Design docs `00` through `06`
- Plans `00` through `06`

---

## Summary

The implementation is clean, well-structured, and faithfully
tracks the approved designs and plans. All 7 commits map 1:1
to the planned commit sequence. The code is consistent, idiomatic
Rust, and correctly handles the wire formats identified in the
design reviews.

One finding: the `periodic get` detail view reads `git_ref`
from the descriptor JSON, but the server serializes it as `ref`
(serde rename). One minor concern about the `roles set` scope
syntax diverging from the design (improvement, not a bug).

**Verdict: One finding. Otherwise approved across all 7 commits.**

---

## Per-Commit Verification

### f91fd2d — Scaffold + Login + Whoami (Plan 00+01)

| Requirement | Status |
|---|---|
| Crate setup, workspace member | ✓ |
| Dependencies (all from design) | ✓ |
| `Config` load/save with XDG, `0600` perms | ✓ |
| No cwd config fallback | ✓ |
| `CbcClient` with `url::Url` base | ✓ |
| `put_json`/`put_empty` split | ✓ |
| `unauthenticated()` constructor | ✓ |
| `Error` enum (no `Auth` variant) | ✓ |
| Debug output via `eprintln!` | ✓ |
| Login flow (health → browser → paste → validate → save) | ✓ |
| Whoami with 401 re-login message using stored host | ✓ |
| `open` crate for browser opening | ✓ |
| LOC: ~451 (plan: ~520) | ✓ |

No issues.

### bbb360b — Build Commands (Plan 02)

| Requirement | Status |
|---|---|
| `BuildDescriptorArgs` shared, `pub`, `version` excluded | ✓ |
| `version` is positional in `BuildNewArgs` | ✓ |
| `SubmitBuildBody` wrapper (`{descriptor, priority}`) | ✓ |
| Nested struct construction (`BuildDestImage`, `BuildTarget`) | ✓ |
| `version_type` parsed via serde (not `"type"`) | ✓ |
| `--all` omits `?user=` filter | ✓ |
| `--limit` client-side truncation | ✓ |
| `build get` with 404/ownership note | ✓ |
| `build revoke` prints server's `detail` on success/409 | ✓ |
| `build components` shows `versions` not `repo` | ✓ |
| LOC: ~525 (plan: ~500) | ✓ |

No issues.

### 53d455c — Build Log Commands (Plan 03)

| Requirement | Status |
|---|---|
| `reqwest-eventsource` + `futures-util` added | ✓ |
| `Logs` variant in `BuildCommands` | ✓ |
| Tail: 4-field response, count context display | ✓ |
| Tail: default `-n 30` matches server | ✓ |
| Follow: SSE with `"done"` event break | ✓ |
| Follow: retry counter, 3-consecutive-error limit | ✓ |
| Follow: `Event::Open` resets counter | ✓ |
| Follow: `es.close()` on max retries | ✓ |
| Follow: fetch final build state after done | ✓ |
| Get: streaming download in chunks | ✓ |
| Get: human-readable size formatting | ✓ |
| LOC: ~320 (plan: ~305) | ✓ |

No issues.

### 6b31a4c — Periodic Build Commands (Plan 04)

| Requirement | Status |
|---|---|
| `--version` as named `#[arg(long)]` (not positional) | ✓ |
| Flattens `BuildDescriptorArgs` from `builds.rs` | ✓ |
| Descriptor → `serde_json::Value` → embed in body | ✓ |
| `cron_expr` and `tag_format` field names correct | ✓ |
| `periodic list` UUID truncation to 8 chars | ✓ |
| `periodic get` combines `last_build_id` + `last_triggered_at` | ✓ |
| `periodic update` all-optional with at least-one check | ✓ |
| `periodic update` fetches existing to merge descriptor | ✓ |
| `enable`/`disable` omit `next_run` from output | ✓ |
| LOC: ~790 (plan: ~450) | See finding |

**Finding F1** (see below).

### 0604d27 — Worker Admin Commands (Plan 05)

| Requirement | Status |
|---|---|
| `resolve_worker_id` with 403 fallback | ✓ |
| Returns `(worker_id, name)` tuple | ✓ |
| `worker list` with BUILD column | ✓ |
| Client-side arch validation | ✓ |
| Worker token displayed prominently | ✓ |
| `regenerate-token` build re-queue warning | ✓ |
| Error handling: 404, 409, 403 | ✓ |
| LOC: ~317 (plan: ~320) | ✓ |

No issues.

### c217529 — Admin Role + Queue (Plan 06 C1)

| Requirement | Status |
|---|---|
| `admin.rs` module with `roles` + `queue` submodules | ✓ |
| `roles list` shows description not caps | ✓ |
| `roles create` body uses `caps` (not `capabilities`) | ✓ |
| `roles update` body includes `name` + `caps` | ✓ |
| `roles delete` with `--force` → `?force=true` | ✓ |
| `roles update` 409 for builtin handled | ✓ |
| `admin queue` shows priority/pending table | ✓ |
| LOC: ~415 (plan: ~350) | ✓ |

No issues.

### 70cf5b8 — Admin User Commands (Plan 06 C2)

| Requirement | Status |
|---|---|
| `Users` variant added to `AdminCommands` | ✓ |
| `users list` from `GET /api/permissions/users` | ✓ |
| `users get` single-request (list + filter) | ✓ |
| Roles with scopes displayed correctly | ✓ |
| Effective caps via per-role GET (N+1) | ✓ |
| `ScopeItem` uses `#[serde(rename = "type")]` | ✓ |
| `ReplaceUserRolesBody` uses `roles` (not `assignments`) | ✓ |
| `users deactivate` shows revocation counts, 409 guard | ✓ |
| `users roles remove` via DELETE path | ✓ |
| LOC: ~510 (plan: ~400) | ✓ |

**Note:** The `roles set` scope syntax diverges from the design's
`--role NAME --scope TYPE=PAT` interleaving. The implementation
uses `--role NAME:TYPE=PAT,TYPE=PAT` compact syntax with
`parse_role_spec`. This is arguably better UX (single flag
per role with inline scopes) and avoids the complex argv-order
parsing the plan described. The wire format is identical. Not a
bug — a pragmatic improvement.

---

## Findings

### F1 — `periodic get` reads `git_ref` from descriptor JSON but server serializes as `ref`

`periodic.rs` line 463:

```rust
let git_ref = c.get("git_ref")?.as_str()?;
```

The server serializes `BuildComponent.git_ref` with
`#[serde(rename = "ref")]` — the JSON key is `"ref"`, not
`"git_ref"`. This line will always return `None`, causing the
component display to show `-` instead of the actual git ref.

The same pattern in `builds.rs` is correct: `build get` parses
the descriptor JSON via `serde_json::from_str::<BuildDescriptor>`
which handles the serde rename internally. But `periodic get`
accesses the raw `serde_json::Value` with string key access and
uses the wrong key.

**Impact:** `cbc periodic get <id>` will display components
without their git refs. Instead of `ceph@v19.2.3` it shows `-`.

**Fix:** Change `c.get("git_ref")` to `c.get("ref")` at
`periodic.rs:463`.

Severity: **Medium.** Display bug only — no data corruption.

---

## Observations

- **Code reuse is excellent.** `periodic.rs` imports
  `BuildDescriptorArgs`, `parse_components`,
  `apply_repo_overrides`, `parse_version_type`, `parse_arch`,
  `parse_priority`, `format_timestamp`, and `WhoamiResponse`
  from `builds.rs`. No duplication.

- **`ScopeItem` serde rename is correct.** `users.rs:130-131`:
  `#[serde(rename = "type")] scope_type: String`. Matches the
  server's `ScopeBody` wire format.

- **`ReplaceUserRolesBody.roles` is correct.** `users.rs:136-138`.
  Matches the server's `ReplaceUserRolesBody`.

- **`CreateRoleBody.caps` is correct.** `roles.rs:104-106`.
  Matches the server's `CreateRoleBody`.

- **`periodic update` descriptor merge is well-implemented.**
  Fetches existing task, extracts each descriptor field from
  the stored JSON, applies overrides from CLI args, constructs
  a new `BuildDescriptor`, serializes to `serde_json::Value`.
  This is the most complex piece of client logic and it's
  correct.

- **`roles set` compact scope syntax** (`NAME:TYPE=PAT`) is a
  UX improvement over the design's interleaved `--role`/`--scope`
  approach. Produces the same wire format. Good call.

- **All `next_run_at` field names in `periodic.rs`.** The
  response types use `next_run_at` — verify this matches the
  server's `PeriodicTaskResponse.next_run`. The server field
  is `next_run: Option<i64>` — the client's `next_run_at` will
  fail to deserialize if the server sends `"next_run"`. However,
  `serde(default)` on most fields means this would silently
  produce `None` rather than an error. **This needs
  verification** — if the server sends `next_run` and the
  client expects `next_run_at`, the field is silently dropped.

  After checking: the server's `PeriodicTaskResponse` has
  `next_run: Option<i64>`. The client's `PeriodicListItem` has
  `next_run_at: Option<i64>`. This is a **field name mismatch**.
  The client should use `next_run` or add `#[serde(alias =
  "next_run")]`.

  **This is a second finding (F2)** — `next_run` displays
  as `-` for all tasks because the field is silently dropped.

---

## Additional Finding

### F2 — `next_run_at` field name mismatch in periodic response types

The server's `PeriodicTaskResponse` sends `"next_run"`. The
client's `PeriodicListItem`, `PeriodicTaskResponse`, and
`PeriodicDetail` all use `next_run_at`. Because these fields
have `#[serde(default)]` or are `Option`, deserialization
succeeds but the value is always `None`.

**Impact:** `cbc periodic list` shows `-` for all next-run
times. `cbc periodic get` shows `next run: -` for all tasks.
`cbc periodic new` never shows the next run time.

**Fix:** Rename `next_run_at` to `next_run` in all three
response structs (`PeriodicTaskResponse`, `PeriodicListItem`,
`PeriodicDetail`).

Severity: **Medium.** Display bug — all next-run times are
missing.

---

## Commit Sizing Summary

| Commit | Plan est. | Actual | Within target |
|---|---|---|---|
| Plan 00 (scaffold) | ~520 | 451 | ✓ |
| Plan 02 (builds) | ~500 | 525 | ✓ |
| Plan 03 (logs) | ~305 | 320 | ✓ |
| Plan 04 (periodic) | ~450 | 790 | Above (update merge logic) |
| Plan 05 (worker) | ~320 | 317 | ✓ |
| Plan 06 C1 (roles) | ~350 | 415 | ✓ |
| Plan 06 C2 (users) | ~400 | 510 | ✓ |

Plan 04 is above the 400-800 guideline at 790 lines. The
excess comes from the `periodic update` descriptor merge logic
(~200 lines of fallback-to-existing field resolution). This is
the most complex command in the CLI and the extra LOC is
justified — the alternative (splitting `update` from the rest)
would create a commit that depends on the subcommand group
from the other half.
