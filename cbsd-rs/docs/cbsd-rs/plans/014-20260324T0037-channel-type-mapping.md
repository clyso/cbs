# 014 — Channel/Type Mapping: Implementation Plan

**Design:**
`docs/cbsd-rs/design/014-20260323T2132-channel-type-mapping.md`
(v3, approved)

## Implementation Notes

From design review N1-N3:

- **N1:** `BuildDescriptor.channel` becomes
  `Option<String>` with `#[serde(default)]` in
  `cbsd-proto`. `None` and `Some("")` both mean
  "use user's default channel."
- **N2:** The permissions API validates at scope
  creation that `channel` scope patterns contain `/`.
  Bare patterns like `"ces"` are rejected.
- **N3:** Any scope access to a channel reveals all
  its types. Type-level filtering within a channel
  list would over-complicate the response for minimal
  security value.

From plan review F1-F5:

- **F1:** Commit 3 expands to fix all consumers of
  `descriptor.channel` across the workspace so every
  commit compiles.
- **F2:** Channel/type resolution is extracted into a
  shared helper function that both `submit_build` and
  `trigger_periodic_build` call. No duplication.
- **F3:** Periodic triggers re-check channel/type
  scopes at trigger time. If a user's scope was
  revoked, the periodic task fails at the next trigger.
- **F4:** `routes/permissions.rs` added to commit 4
  for `KNOWN_CAPS` registration.
- **F5:** `extractors.rs` removed from commit 5 —
  existing `scope_pattern_matches` handles the new
  format. Scope value construction and pattern
  validation are in `routes/builds.rs` and
  `routes/permissions.rs`.

## Commit Breakdown

8 commits, ordered by dependency.

---

### Commit 1: `cbsd-rs/docs: add channel/type mapping design and plan`

**Documentation only**

Design (v3), this plan, and all design/plan reviews.

---

### Commit 2: `cbsd-rs: add channel and channel_type database schema`

**~300 authored lines**

Migration, DB CRUD module, and query changes. All
existing callers of `insert_build` pass `None` for
the new channel parameters — the feature is wired up
in commit 5.

**Files:**

| File | Change |
|------|--------|
| `migrations/005_channels.sql` | New: `channels`, `channel_types` tables with soft-delete indexes; `ALTER TABLE users ADD COLUMN default_channel_id`; `ALTER TABLE builds ADD COLUMN channel_id, channel_type_id` |
| `cbsd-server/src/db/channels.rs` | New: create/get/list/update/soft-delete for channels and types; `set_default_type`; `resolve_channel_type` (lookup by name pair, returns project + prefix_template) |
| `cbsd-server/src/db/users.rs` | `get_user` / `create_or_update_user` return `default_channel_id`; new `set_default_channel` |
| `cbsd-server/src/db/builds.rs` | `insert_build` accepts optional `channel_id: Option<i64>` and `channel_type_id: Option<i64>`; `BuildRecord` and `BuildListRecord` gain `channel_id`, `channel_type_id`, `channel_name`, `channel_type_name` via LEFT JOIN; existing callers pass `None` |
| `cbsd-server/src/db/mod.rs` | Register `channels` module |
| `cbsd-server/src/routes/builds.rs` | Update `insert_build_internal` call to pass `None, None` for new params |
| `.sqlx/` | Regenerated |

**Key details:**

- `channels.name` unique among active (partial index).
- `channel_types.type_name` has CHECK constraint
  for dev/release/test/ci.
- Soft-delete sets `deleted_at`; application clears
  `default_type_id` when type is soft-deleted.
- `resolve_channel_type(channel_name, type_name)`
  returns `(channel_id, channel_type_id, project,
  prefix_template)` for the submission flow.
- `insert_build` takes the new params as `Option` so
  existing callers compile with `None`.

**Validation:**

```bash
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx database create
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx migrate run
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx prepare --workspace
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

---

### Commit 3: `cbsd-rs: make channel optional in BuildDescriptor`

**~60 authored lines**

`BuildDescriptor.channel` becomes `Option<String>`
with `#[serde(default, skip_serializing_if = "Option::is_none")]`.
All consumers across the workspace are updated with
minimal bridging to maintain compilation.

**Files:**

| File | Change |
|------|--------|
| `cbsd-proto/src/build.rs` | `channel: Option<String>`; update tests |
| `cbc/src/builds.rs` | Wrap channel in `Some(...)` at construction; `.as_deref().unwrap_or("")` at display |
| `cbc/src/periodic.rs` | Same bridging as builds.rs |
| `cbsd-server/src/routes/builds.rs` | Handle `Option` in scope check (pass `.unwrap_or_default()` or skip if None — full logic comes in commit 5) |
| `cbsd-server/src/scheduler/tag_format.rs` | Unwrap `Option` for tag interpolation |

**Validation:**

```bash
cargo build --workspace
cargo test --workspace
```

---

### Commit 4: `cbsd-rs/server: add channel CRUD REST endpoints`

**~500 authored lines**

Route handlers for channel and type management.

**Files:**

| File | Change |
|------|--------|
| `cbsd-server/src/routes/channels.rs` | New: channel CRUD (create, list, get, update, soft-delete); type CRUD (add, update, soft-delete, set-default); scoped visibility for reads |
| `cbsd-server/src/routes/admin.rs` | `PUT /api/admin/users/{email}/default-channel` handler |
| `cbsd-server/src/routes/permissions.rs` | Add `channels:manage` and `channels:view` to `KNOWN_CAPS`; validate channel scope patterns contain `/` at creation |
| `cbsd-server/src/app.rs` | Register `/channels` router |
| `cbsd-server/src/db/seed.rs` | Add `channels:manage` and `channels:view` to builtin admin role caps |

**Key details:**

- Write endpoints require `channels:manage`.
- Read endpoints available to all authenticated users;
  list filters by channel scope access.
- Any scope access to a channel (e.g., `ces/dev`)
  reveals all its types in the response.
- Channel list response includes types inline.
- First type added to a channel auto-sets
  `default_type_id`.
- `KNOWN_CAPS` updated so custom roles can be assigned
  channel capabilities.

**Validation:**

```bash
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

If commit exceeds 800 lines, split: channel CRUD in
one commit, type CRUD + admin + permissions in a
second.

---

### Commit 5: `cbsd-rs/server: resolve channel/type in build submission`

**~300 authored lines**

The core feature: intercept build submission, resolve
channel/type, rewrite `dst_image.name`, validate
scopes. The resolution logic is extracted into a
shared helper for reuse by the periodic scheduler.

**Files:**

| File | Change |
|------|--------|
| `cbsd-server/src/channels/mod.rs` | New: `resolve_and_rewrite(state, descriptor, user)` — shared helper that resolves channel/type, validates scopes, rewrites `dst_image.name`, returns `(channel_id, channel_type_id)` |
| `cbsd-server/src/routes/builds.rs` | Call `resolve_and_rewrite` in `submit_build` / `insert_build_internal`; pass resolved IDs to `insert_build`; remove registry scope check; construct scope value as `"{channel}/{type}"` |
| `cbsd-server/src/routes/permissions.rs` | Validate channel scope patterns contain `/` at creation time (if not already done in commit 4) |
| `cbsd-proto/src/build.rs` | Deprecate `registry_host()` |

**Key details:**

- `resolve_and_rewrite()` is the single source of
  truth for channel resolution. Both manual submission
  and periodic trigger call it.
- Empty/absent channel → user's `default_channel_id`.
  If NULL, reject: "no default channel assigned."
- Type from `descriptor.version_type`. If not found
  for channel, reject. If omitted, use channel's
  default type. If no default type, reject.
- Prefix template: `${username}` resolved from email
  prefix (part before `@`).
- Scope check: `"{channel_name}/{type_name}"` against
  user's channel scope patterns.
- Rewrite: `dst_image.name` becomes
  `<project>/<prefix>/<original_image_name>`.
- Registry scope check removed from submission.

**Validation:**

```bash
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

---

### Commit 6: `cbsd-rs/server: resolve channel/type in periodic scheduler`

**~80 authored lines**

The scheduler trigger path calls the same
`resolve_and_rewrite` helper, including scope
re-validation.

**Files:**

| File | Change |
|------|--------|
| `cbsd-server/src/scheduler/trigger.rs` | Call `resolve_and_rewrite` before `insert_build_internal`; load task owner's user record for scope checking |

**Key details:**

- Resolution happens at each trigger, not at task
  creation time. Channel renames or type deletions
  cause trigger failure.
- Scopes are re-checked at trigger time. If the
  task owner's `channel/type` scope was revoked, the
  trigger fails. This prevents stale permissions from
  producing builds.
- Failure follows the existing retry/disable path
  (exponential backoff, disable after 10 retries).

**Validation:**

```bash
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

---

### Commit 7: `cbc: add channel commands and optional channel/type flags`

**~400 authored lines**

Client-side changes: optional `--channel`, `--type`,
new `cbc channel` commands.

**Files:**

| File | Change |
|------|--------|
| `cbc/src/builds.rs` | `--channel` optional (remove required); `--type` optional |
| `cbc/src/channels.rs` | New: `cbc channel list` |
| `cbc/src/admin/channels.rs` | New: admin channel/type CRUD commands |
| `cbc/src/admin/mod.rs` | Register channel admin subcommands |
| `cbc/src/main.rs` | Register `Channel` command |

**Key details:**

- `cbc channel list` shows channels with types,
  highlighting the user's default channel and each
  channel's default type.
- `cbc admin channel create <name>` creates a channel.
- `cbc admin channel type add <channel> <type>
  --project <project> [--prefix <template>]` adds a
  type.
- `cbc admin user set-default-channel <email>
  <channel>` assigns a user's default.

**Validation:**

```bash
cargo build --workspace
cargo test --workspace
```

---

### Commit 8: `cbsd-rs/docs: add implementation reviews`

**Documentation only**

Post-implementation review documents.

---

## Dependency Graph

```
Commit 1 (docs)
    ↓
Commit 2 (DB schema + CRUD)
    ↓
Commit 3 (proto + consumer bridging)
    ↓
Commit 4 (channel REST endpoints)
    ↓
Commit 5 (resolution helper + build submission)
    ↓
Commit 6 (scheduler resolution + scope recheck)
    ↓
Commit 7 (cbc client)
    ↓
Commit 8 (impl reviews)
```

Commits 2-3 are foundation. Commit 4 requires 2
(DB queries). Commit 5 requires 3 (optional channel)
and 4 (channel lookup). Commit 6 requires 5
(shared resolution helper). Commit 7 requires 3
(proto change) and 4 (channel API exists).

## Sizing Notes

Commit 4 (~500 lines) is the largest. If it exceeds
800 lines, split at channel CRUD vs type CRUD + admin.

Commit 5 (~300 lines) must not be split — resolution,
rewriting, and scope checking are one logical unit.

## Progress

| # | Commit | Status |
|---|--------|--------|
| 1 | docs | Pending |
| 2 | DB schema + CRUD | Pending |
| 3 | proto + consumer bridging | Pending |
| 4 | channel REST endpoints | Pending |
| 5 | resolution helper + build submission | Pending |
| 6 | scheduler resolution + scope recheck | Pending |
| 7 | cbc client | Pending |
| 8 | impl reviews | Pending |
