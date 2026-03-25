# 014 — Channel/Type Image Destination Mapping

## Status

Draft v3 — addresses review v2

## Problem

Container images are pushed to wherever the user specifies
in `dst_image.name`. There are no guard-rails: a typo in
the image name pushes to the wrong project, a user can
push to any Harbor project they know the name of, and
there is no relationship between the build's "channel"
and where its artifacts actually land. The channel field
is a free-form string with no server-side enforcement.

Operators need channels to be meaningful: a build for
channel "ces" type "release" must land in a specific
Harbor project, and only users with the right permissions
can produce builds for that channel/type combination. The
image destination must be deterministic from the channel
and type, not from user input.

## Design

### Concepts

**Channel** — a named grouping that represents a
destination context (e.g., "ces", "ccs", "user",
"customer"). Channels are managed by admins via the REST
API and stored in the database.

**Type** — a build classification within a channel.
Type names must be one of the four `VersionType` enum
values: `dev`, `release`, `test`, `ci`. Types are
per-channel: channel "ces" may allow dev/release/ci,
while channel "user" may only allow dev. Each
channel/type pair maps to a Harbor **project** and an
optional **prefix template**.

The `channel_types.type_name` column has a CHECK
constraint restricting it to the four valid values.
Custom type names are not supported — this avoids wire
format changes to `BuildDescriptor.version_type` and
any cbscore modifications.

**Mapping chain:**

```
user request
  → channel (explicit or user's default)
  → type (explicit or channel's default)
  → (project, prefix_template) from DB
  → dst_image.name = <project>/<prefix>/<image>
  → descriptor passed to cbscore
```

### Image Path Construction

The server rewrites `dst_image.name` to contain the
project, prefix, and image name. The registry is NOT
included in `dst_image.name` — it is supplied by the
worker's cbscore config independently (see "Registry"
section below).

Given:


- Channel "user", type "dev" maps to project
  `cbs-i<joao.luis@clyso.com>{username}`
- User "<joao.luis@clyso.com>" builds "ceph" with tag
  "v19.2.2"


The server rewrites `dst_image.name` to:

```
cbs-internal/joao.luis/ceph
```


cbscore on the worker side prepends the registry from
its own config, producing the final push URI:

```
harbor.clyso.com/cbs-internal/joao.luis/ceph:v19.2.2

```

The path components:

```
<project>/<prefix>/<image-name>:<tag>
    │         │         │         │
    │         │         │         └ from descriptor or
    │         │         │           user override (--image-tag)
    │         │         └ from descriptor or user
    │         │           override (--image-name)
    │         └ resolved from prefix_template
    └ from channel/type mapping
```

`--image-name` and `--image-tag` on `cbc` override only
the image name and tag portions. The project and prefix
are always enforced by the server based on the
channel/type mapping. The user cannot bypass this.

**Multi-segment image names:** the default `--image-name`
is `ceph/ceph`, which produces paths like
`cbs-internal/joao.luis/ceph/ceph`. This is valid in
Harbor but produces 4 path segments. Documented here for
operator awareness — the default image name is a `cbc`
client concern and can be changed independently.

### Registry

The registry host is NOT controlled by the server or
the channel mapping. It comes from the worker's cbscore
config (`config.storage.registry.url`). The server does
not validate or enforce the registry.

**Deferred:** registry enforcement requires changes to
how cbscore handles registry configuration. Currently,
cbscore receives the image name from the descriptor and
the registry from its own config, constructing the push
URI as `{registry}/{name}:{tag}`. To enforce the
registry server-side, either the descriptor would need
a separate registry field or the cbscore wrapper would
need to parse the registry from the image name. Both
require cbscore-level changes that are outside the
scope of this design.

### Prefix Templates

The `prefix_template` field supports variable
substitution. Initially only `${username}` is supported:

| Variable | Resolves to |
|----------|-------------|
| `${username}` | Email prefix of the authenticated user (part before `@`) |

An empty prefix template means no prefix — the image
name goes directly under the project.

The template engine is deliberately simple (string
replacement, no conditionals or nesting). Adding new
variables in the future requires a code change to
register the variable, but no schema change.

### Channel Management

Channels are managed entirely through the REST API by
admins. There is no server config for channels — no
`channels:` section in `server.yaml`.

**First-startup experience:**

1. Server starts, DB is empty — no channels, no types
2. Admin logs in, creates channels via
   `cbc channel create ...`
3. Admin adds types to channels via
   `cbc channel type add ...`
4. Admin assigns default channels to users via
   `cbc admin user set-default-channel ...`
5. Users can now submit builds

If a user submits a build before their default channel
is assigned, the server returns a clear error: "no
default channel assigned — contact your administrator."

**Per-channel default type** — each channel has a
`default_type_id` pointing to one of its types. When
the user omits `--type`, the channel's default type is
used. The first type added to a channel becomes the
default; admins can change it explicitly.

When a type is soft-deleted, the application clears
`default_type_id` if it pointed to the deleted type.
If a channel has no default type and the user omits
`--type`, the server returns: "channel has no default
type; specify --type explicitly."

### Build Submission Flow

1. `cbc build new v19.2.2 --component ceph@v19.2.2`
   (no `--channel`, no `--type`)

2. Client sends descriptor with `channel: ""` (or
   absent), `version_type: "dev"` (cbc default).

3. Server receives the submission:
   a. Channel is empty → resolve from user's
      `default_channel_id` → e.g., "user" (ID 3)
   b. Type is "dev" → look up channel_type row for
      (channel_id=3, type_name="dev")
   c. Validate user has scope for "user/dev"
   d. Found: project="cbs-internal",
      prefix_template="${username}"
   e. Resolve prefix: `${username}` →
      `joao.luis` (from user email)
   f. Rewrite `dst_image.name`:
      `cbs-internal/joao.luis/ceph`
   g. Store resolved channel_id and channel_type_id
      in the build record
   h. Pass rewritten descriptor to cbscore

4. If the user explicitly passes `--channel ces
   --type release`:
   a. Look up (channel="ces", type="release") →
      project="ces-release", prefix=""
   b. Validate user has scope for `ces/release`
   c. Rewrite `dst_image.name`:
      `ces-release/ceph`

### RBAC: Channel/Type Scopes

The existing scope system (`user_role_scopes` table
with `scope_type` and `pattern`) is extended.

Channel scope patterns use `channel/type` format:

- `ces/dev` — exact match: channel "ces", type "dev"
- `ces/*` — all types in channel "ces"
- `*` — all channels and types (admin)

All channel scope patterns must contain a `/`. Bare
channel names (e.g., `"ces"`) are not supported. This
is a fresh deployment with no existing scope data to
migrate.

**Scope validation happens for all builds**, regardless
of whether the channel was explicitly specified or
resolved from the user's default. The server checks
the resolved `channel/type` pair against the user's
scope assignments after resolution.

**Registry scope check removed.** The current
`submit_build` handler checks `registry_host()` from
`dst_image.name` against registry scopes. With the
new image path format (`<project>/<prefix>/<image>`),
`registry_host()` returns the project name, not a
registry. The registry scope check is removed from
build submission. The `registry` scope type remains
in the schema for potential future use but is not
checked at submission. The `registry_host()` method
on `BuildDescriptor` is deprecated.

The `repository` scope type also remains in the
schema for future use.

### Database Schema

```sql
CREATE TABLE IF NOT EXISTS channels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    default_type_id INTEGER
                REFERENCES channel_types(id),
    deleted_at  INTEGER,
    created_at  INTEGER NOT NULL
                DEFAULT (unixepoch()),
    updated_at  INTEGER NOT NULL
                DEFAULT (unixepoch())
);

CREATE UNIQUE INDEX idx_channels_name_active
    ON channels(name) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS channel_types (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id       INTEGER NOT NULL
                     REFERENCES channels(id)
                     ON DELETE CASCADE,
    type_name        TEXT NOT NULL
                     CHECK (type_name IN
                       ('dev','release','test','ci')),
    project          TEXT NOT NULL,
    prefix_template  TEXT NOT NULL DEFAULT '',
    deleted_at       INTEGER,
    created_at       INTEGER NOT NULL
                     DEFAULT (unixepoch()),
    updated_at       INTEGER NOT NULL
                     DEFAULT (unixepoch())
);

CREATE UNIQUE INDEX idx_channel_types_active
    ON channel_types(channel_id, type_name)
    WHERE deleted_at IS NULL;
```

**Users table addition:**

```sql
ALTER TABLE users ADD COLUMN default_channel_id
    INTEGER REFERENCES channels(id)
    ON DELETE SET NULL;
```

When `default_channel_id` is NULL (channel deleted or
not yet assigned), builds without an explicit
`--channel` are rejected with a clear error.

**Builds table additions:**

```sql
ALTER TABLE builds ADD COLUMN channel_id
    INTEGER REFERENCES channels(id);
ALTER TABLE builds ADD COLUMN channel_type_id
    INTEGER REFERENCES channel_types(id);
```

### Soft Delete (Tombstones)

Channels and types are soft-deleted (`deleted_at` set
to a timestamp). This preserves FK references from
existing build records and allows name reuse after
deletion. The partial unique index ensures only one
active channel or type can have a given name.

When a type is soft-deleted, the application checks
whether the owning channel's `default_type_id` points
to the deleted type. If so, it sets
`default_type_id = NULL`.

Soft-deleted channels and types are excluded from all
API responses and resolution logic. They exist only
for referential integrity.

### Build Record Display

Build list and get responses include denormalized
channel and type names alongside the FK IDs. The route
handler JOINs `channels.name` and
`channel_types.type_name` into the response:

```json
{
  "id": 42,
  "channel_id": 3,
  "channel_name": "user",
  "channel_type_id": 7,
  "channel_type_name": "dev",
  ...
}
```

This avoids requiring clients to make a second API call
to resolve IDs to names for display.

### Periodic Builds

Periodic tasks store a descriptor with `channel` and
`version_type` as logical names. The same resolution
logic applies at trigger time — the scheduler resolves
channel/type from the descriptor just like a manual
submission.

If a channel is renamed, periodic tasks referencing the
old name will fail at trigger time (the old name no
longer resolves). The admin must update the periodic
task's descriptor. If a channel is deleted, the periodic
task fails, retries, and is eventually disabled by the
existing retry/disable mechanism.

Periodic tasks do not store `channel_id` or
`channel_type_id` — they store logical names and
resolution happens at each trigger.

### REST API

#### Channels CRUD

```
POST   /api/channels              Create channel
GET    /api/channels              List channels with types
GET    /api/channels/{id}         Get channel with types
PUT    /api/channels/{id}         Update (name, desc)
DELETE /api/channels/{id}         Soft-delete
```

Write endpoints (POST, PUT, DELETE) require
`channels:manage`.

Read endpoints (GET list, GET detail) are available to
all authenticated users. The list endpoint includes
types per channel in the response and filters results
to channels the user has scope access to. The detail
endpoint is available to users who have any scope for
that channel (e.g., `ces/*` grants access to view
channel "ces" and all its types).

This allows `cbc channel list` to show channels with
their available types without requiring admin
privileges.

#### Channel Types CRUD

```
POST   /api/channels/{id}/types          Add type
PUT    /api/channels/{id}/types/{tid}    Update type
DELETE /api/channels/{id}/types/{tid}    Soft-delete
PUT    /api/channels/{id}/default-type   Set default
```

All type write endpoints require `channels:manage`.
Type listing is included in the channel GET responses
(no separate list endpoint needed).

#### User Default Channel

```
PUT /api/admin/users/{email}/default-channel
```

Body: `{"channel_id": 3}`

Requires `permissions:manage`.


### cbc Changes

`cbc build new` evolves:

- `--channel` (`-p`) becomes optional (default: omitted
  → server resolves from user's default)
- `--type` becomes optional (default: omitted → server
  uses channel's default type)

- `--image-name` overrides only the image name portion
  (not project/prefix)
- `--image-tag` overrides the tag

New commands:

- `cbc channel list` — list channels the user has
  access to, with their types
- `cbc admin channel create/delete/update` — admin
  channel management
- `cbc admin channel type add/remove/update` — admin
  type management
- `cbc admin user set-default-channel` — set user's
  default channel

### cbscore Impact

The server rewrites `dst_image.name` to
`<project>/<prefix>/<image>` before the descriptor
reaches cbscore. cbscore receives this and constructs
the push URI as `{registry}/{dst_image.name}:{tag}`
using its own configured registry. No cbscore code
changes are needed.

### Migration

This is a fresh deployment — no existing data to
migrate. The design assumes no prior build records,
channel scopes, or user assignments exist. All
channels, types, and assignments are created via the
API after first startup.

## Files Changed

| File | Change |
|------|--------|
| `migrations/005_channels.sql` | New: channels, channel_types tables; users.default_channel_id; builds columns |
| `cbsd-server/src/db/channels.rs` | New: channel + type CRUD queries |
| `cbsd-server/src/db/users.rs` | default_channel_id field |
| `cbsd-server/src/db/builds.rs` | channel_id, channel_type_id; JOIN names in responses |
| `cbsd-server/src/routes/channels.rs` | New: channel + type CRUD endpoints |
| `cbsd-server/src/routes/builds.rs` | Channel/type resolution in submit_build; remove registry scope check |
| `cbsd-server/src/auth/extractors.rs` | Channel/type scope pattern matching (channel/type format) |
| `cbsd-proto/src/build.rs` | Deprecate `registry_host()` |
| `cbsd-server/src/app.rs` | Register channels router |
| `cbc/src/builds.rs` | --channel optional, --type optional |
| `cbc/src/channels.rs` | New: channel list + admin commands |
| `cbc/src/main.rs` | Register channel subcommands |
| `.sqlx/` | Regenerated |

## Open Questions

1. **Should channels have an `owner` field?** Deferred
   unless needed.

2. **Per-channel default image name?** Deferred to
   avoid scope creep.
