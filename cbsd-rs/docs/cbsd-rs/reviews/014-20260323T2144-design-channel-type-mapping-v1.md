# 014 — Design Review: Channel/Type Image Destination Mapping (v1)

**Design:**
`docs/cbsd-rs/design/014-20260323T2132-channel-type-mapping.md`

**Verdict:** Needs revision — one critical flaw, several
important gaps

---

## Problem Statement

Validated. The problem is real and accurately described:

- `dst_image.name` is user-controlled and flows through
  to cbscore without server-side enforcement. Confirmed
  at `cbc/src/builds.rs:254-255` (client sets it from
  `--image-name`, default `ceph/ceph`) and
  `routes/builds.rs:121` (server does not modify it).
- The `channel` field is a free-form string checked only
  against scope patterns — no table of valid channels
  exists. Confirmed at `extractors.rs:100`.
- cbscore pushes to whatever image path it constructs
  from `VersionImage(registry, name, tag)`. Confirmed
  at `containers.py:24`.

The design's motivation is sound: making image
destinations deterministic from channel+type removes a
class of operator errors and enables meaningful
authorization.

---

## Findings

### F1 — Critical: image path construction breaks cbscore

The design says the server rewrites `dst_image.name` to
the full path including registry:

```
dst_image.name = harbor.clyso.com/cbs-internal/joao.luis/ceph
```

And claims "cbscore Impact: None."

**This is wrong.** The cbscore wrapper at
`scripts/cbscore-wrapper.py:182-183` passes the registry
and image name as **separate** arguments to cbscore:

```python
version_create_helper(
    ...
    registry=config.storage.registry.url,  # from worker config
    image_name=_get_str(dst_image, "name"),  # from descriptor
    ...
)
```

cbscore constructs the push URI as
`{registry}/{name}:{tag}` (`containers.py:24`). If
`dst_image.name` contains the registry, the result is a
double-registry URI:

```
harbor.clyso.com/harbor.clyso.com/cbs-internal/joao.luis/ceph:tag
```

**Options to fix:**

**(a)** Server writes only `<project>/<prefix>/<image>`
into `dst_image.name` (no registry). The worker's
cbscore config supplies the registry as it does today.
Simplest, no wrapper changes. But the server can't
verify the push target matches `allowed-registries`
because the registry comes from the worker's config,
not the descriptor.

**(b)** Modify the wrapper to parse registry out of
`dst_image.name` and pass it to cbscore instead of the
config value. Requires wrapper changes (contradicts
"cbscore Impact: None" but the wrapper is cbsd-rs
code, not cbscore itself).

**(c)** Add a `registry` field to `BuildDestImage` in
`cbsd-proto`. The server sets it from the channel
config; the wrapper passes it directly to cbscore.
Cleanest wire format but requires proto + wrapper
changes.

The design must pick one and update the image path
construction section, the cbscore impact statement,
and (if option b/c) the files-changed table.

### F2 — Important: VersionType enum vs. free-form type_name

`BuildDescriptor.version_type` is a `VersionType` enum
with four variants: `Release`, `Dev`, `Test`, `Ci`
(`cbsd-proto/src/build.rs:70-77`).

The design's `channel_types.type_name` is a free-form
`TEXT` column. The design's examples use "dev",
"release", "ci" — matching the enum values. But if
types are per-channel and managed in the DB, two
questions arise:

1. **Must `type_name` match a `VersionType` variant?**
   If yes, the DB should have a CHECK constraint, and
   per-channel types are just a subset selection from
   the four fixed values. The design should say this
   explicitly.

2. **Can channels define custom types?** If yes,
   `version_type` must change from an enum to a String
   in `cbsd-proto`, which is a breaking wire-format
   change affecting workers and the wrapper (cbscore's
   `version_create_helper` validates the type name
   against its own enum at
   `cbscore/versions/create.py:197-202`).

The design doesn't address this. It must state which
model applies.

### F3 — Important: channel name migration

Current `channel` values are strings like `"ces-devel"`
that encode both channel and type information (the
scope design doc shows `"ces-devel/*"` as an example
pattern). The new model separates channel (`"ces"`)
from type (`"dev"`).

This means:

- **Existing build records** have `channel: "ces-devel"`
  in their descriptor JSON. The new `builds.channel_id`
  and `builds.channel_type_id` columns won't be
  populated for old builds. The design should clarify
  whether old builds are backfilled or left with NULL
  IDs.

- **Existing scope patterns** like `"ces-devel"` or
  `"ces-devel/*"` must be migrated to the new
  `"channel/type"` format. The design mentions
  backwards compat ("patterns without `/` are treated
  as `channel/*`") but this only works if channel
  names don't change. If `"ces-devel"` becomes
  `"ces"`, the old pattern `"ces-devel"` matches
  nothing in the new world.

The design needs a migration section covering scope
pattern rewriting and existing build record handling.

### F4 — Important: default_type_id FK and soft deletes

`channels.default_type_id` references
`channel_types(id)` with `ON DELETE SET NULL`. But
channel types are **soft-deleted** (set `deleted_at`,
row remains). A soft-deleted type doesn't trigger
`ON DELETE SET NULL`, so `default_type_id` can point
to a soft-deleted type.

The resolution logic must check whether the default
type is still active. The design should specify this
behavior — either:

- Clear `default_type_id` as part of the soft-delete
  operation (application-level), or
- The resolution logic skips soft-deleted defaults and
  falls back (but to what?).

### F5 — Important: seed data not specified

The server panics at startup if `default-channel`
doesn't exist in the DB. On first startup, the DB is
empty. `seed.rs` must create the default channel with
at least one type.

The design lists `seed.rs` in files-changed but doesn't
specify:
- What channel(s) to seed
- What types per channel (names, projects, prefix
  templates)
- Whether to seed the admin user's default_channel_id

Without this, the first-startup experience is broken.

### F6 — Moderate: Open Question 2 should be a decision

> Should the build list/get responses include the
> resolved channel and type names (not just IDs)?

Yes — this must be decided now, not deferred. Without
names in the response, every `cbc build list` display
requires a second API call (or the client must cache
channel metadata). The `WorkerInfoResponse` pattern
(inline denormalized fields) is the right model. The
schema only has FK IDs; the route handler should JOIN
and include `channel_name` and `type_name`.

This affects the API contract and should be in the
design, not an open question.

### F7 — Moderate: periodic builds interaction

The scheduler currently interpolates `{channel}` from
`descriptor.channel` in tag format templates
(`scheduler/tag_format.rs:131`). With the new model:

- Does the periodic task store a channel_id or a
  channel name?
- If the channel is renamed or soft-deleted, what
  happens to periodic tasks referencing it?
- Should periodic tasks also store a `channel_type_id`?

The design doesn't mention periodic builds at all.

### F8 — Minor: multi-segment image names

The default `--image-name` is `ceph/ceph`. With the
new path construction:

```
<project>/<prefix>/<image-name>
cbs-internal/joao.luis/ceph/ceph
```

This produces four path segments under the registry,
which is valid in Harbor but might surprise operators
who expect three. The design should document this
explicitly or reconsider whether the default should
change to a single-segment name.

---

## Deferred Items (flagged for user)

The design marks these as deferred or open:

1. **`cbc channel list` command** (Open Q1) — reasonable
   deferral, but should be part of the initial plan
   since users need channel discoverability.

2. **Channel owner field** (Open Q3) — reasonable
   deferral.

3. **Per-channel default image name** (Open Q4) —
   reasonable deferral. The current `--image-name`
   default on the client side is sufficient.

---

## What's Done Well

**Core mapping model.** Channel → types → (project,
prefix_template) is the right abstraction. It cleanly
separates authorization (who can use which channel/type)
from routing (where artifacts land).

**Soft deletes with partial unique indexes.** Allowing
name reuse after deletion while preserving FK integrity
is the correct approach for a system with long-lived
build records.

**Registry guard-rail.** The `allowed-registries`
config is a good defense-in-depth measure even though
the registry is currently always the same.

**Prefix templates.** `${username}` for per-user
namespacing is a practical starting point. Keeping the
engine simple (string replacement, no conditionals) is
the right call.

**Server-side resolution.** Making the client dumb
(send channel name or nothing, server fills in
everything) is the right architecture. It centralizes
policy and simplifies the client.

---

## Summary

| Severity | # | Finding |
|----------|---|---------|
| Critical | 1 | F1: image path construction double-registry |
| Important | 4 | F2: VersionType vs type_name; F3: migration; F4: soft-delete FK; F5: seed data |
| Moderate | 2 | F6: Open Q2 must be decided; F7: periodic builds |
| Minor | 1 | F8: multi-segment image names |

F1 is a design-level flaw that affects the core image
path construction. It must be resolved before the
design can be planned. F2-F5 are gaps that need
answers in the design document. F6-F7 are scope
questions that should be addressed before implementation
begins.
