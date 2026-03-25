# 04 — Periodic Builds

## Overview

CRUD management of cron-scheduled periodic build tasks.

## Commands

### `cbc periodic new [options]`

Create a new periodic build task.

```
$ cbc periodic new \
    --cron "0 2 * * *" \
    --tag-format "{version}-nightly-{DT}" \
    --version 19.2.3 \
    --channel ces \
    --component ceph@v19.2.3 \
    --type dev \
    --summary "Nightly CES build"

periodic task 550e8400-... created (enabled)
  schedule: 0 2 * * * (daily at 02:00 UTC)
  next run: 2026-03-19 02:00:00 UTC
```

**Endpoint:** `POST /api/periodic`

**Requires:** `periodic:create` + `builds:create`
(with full scope validation against the descriptor).
A user with `periodic:create` but not `builds:create`
gets 403.

**Request body:**

```json
{
  "cron_expr": "0 2 * * *",
  "tag_format": "{version}-nightly-{DT}",
  "descriptor": { ... BuildDescriptor as JSON ... },
  "priority": "normal",
  "summary": "Nightly CES build"
}
```

Note the field names: `cron_expr` and `tag_format`
(not `cron` or `tag`). The `descriptor` field is a
raw JSON object (serialized `BuildDescriptor`).

The client constructs a `BuildDescriptor` struct from
the CLI options (same as `build new`), serializes it
to `serde_json::Value`, and embeds it as the
`descriptor` field in the POST body.

**Periodic-specific options:**

| Flag | Description | Default |
|---|---|---|
| `--cron` | Cron expression (5-field) | required |
| `--tag-format` | Tag format with `{var}` | required |
| `--summary` | Description | none |
| `--priority` | Build priority | `normal` |

**Build descriptor options** (shared with `build new`
via `#[command(flatten)]`):

| Flag | Description | Default |
|---|---|---|
| `--version` | Version string | required |
| `--channel` | Release channel | required |
| `--component` | `name@gitref` (repeat) | required |
| `--type` | Version type | `dev` |
| `--repo-override` | `name=url` | none |
| `--distro` | Distribution | `rockylinux` |
| `--os-version` | OS version | `el9` |
| `--image-name` | Image name | `ceph/ceph` |
| `--image-tag` | Image tag (template) | VERSION |
| `--arch` | Architecture | `x86_64` |

### `cbc periodic list`

List all periodic tasks.

```
$ cbc periodic list

  ID        ENABLED  SCHEDULE     NEXT RUN
  550e8400  yes      0 2 * * *    2026-03-19 02:00
  660f9500  no       0 0 * * 1    -
```

**Endpoint:** `GET /api/periodic`

Tabular output. UUIDs truncated to 8 hex chars.
`NEXT RUN` is `-` when disabled.

All response timestamps are Unix epoch integers —
the client converts to human-readable UTC strings.

### `cbc periodic get <id>`

Show details of a single periodic task.

```
$ cbc periodic get 550e8400-...

          id: 550e8400-...
        cron: 0 2 * * *
  tag format: {version}-nightly-{DT}
     enabled: yes
  created by: admin@clyso.com
    next run: 2026-03-19 02:00:00 UTC
     retries: 0
  last error: -
  last build: #42 at 2026-03-18 02:00

  descriptor:
     version: 19.2.3
     channel: ces
        type: dev
       image: ceph/ceph:{version}-nightly-{DT}
       comps: ceph@v19.2.3
      distro: rockylinux el9
    priority: normal
     summary: Nightly CES build
```

**Endpoint:** `GET /api/periodic/{id}`

The "last build" line combines `last_build_id` with
`last_triggered_at` (trigger time, not build completion
time).

### `cbc periodic update <id> [options]`

Update an existing periodic task.

```
$ cbc periodic update 550e8400-... \
    --cron "0 3 * * *" \
    --summary "Nightly CES (03:00)"

periodic task 550e8400-... updated
```

**Endpoint:** `PUT /api/periodic/{id}`

**Requires:** `periodic:manage`. If updating the
descriptor, additionally requires `builds:create` with
full scope validation.

All options are optional (at least one required). Same
option set as `periodic new` — including `--tag-format`.
Any provided option overwrites the existing value.

### `cbc periodic delete <id>`

Delete a periodic task permanently.

```
$ cbc periodic delete 550e8400-...
periodic task 550e8400-... deleted
```

**Endpoint:** `DELETE /api/periodic/{id}`

Deleting a task does not cancel builds already triggered
by it.

### `cbc periodic enable <id>`

Re-enable a disabled periodic task.

```
$ cbc periodic enable 550e8400-...
periodic task 550e8400-... enabled
```

**Endpoint:** `PUT /api/periodic/{id}/enable`

The server response has no `next_run` field. To show
the next scheduled time, the client would need a
follow-up `GET /api/periodic/{id}`. For simplicity,
the enable command omits next_run from its output.

### `cbc periodic disable <id>`

Disable an active periodic task.

```
$ cbc periodic disable 550e8400-...
periodic task 550e8400-... disabled
```

**Endpoint:** `PUT /api/periodic/{id}/disable`

## Tag format variables

Displayed in `--help` for `periodic new` and
`periodic update`:

**Time** (UTC, at trigger):
`{Y}` `{m}` `{d}` `{H}` `{M}` `{S}` `{DT}`

**Build descriptor:**


- `{version}` — `--version` value
- `{base_tag}` — `--image-tag` value (the template tag
  before interpolation)
- `{channel}` — `--channel` value
- `{user}` — build creator's display name
- `{arch}` — `--arch` value
- `{distro}` — `--distro` value
- `{os_version}` — `--os-version` value

## Error handling

- Invalid cron expression → 400, print server message.
- Unknown tag format variable → 400, print server
  message with list of unknown variables.
- Missing required options → clap handles this.
- Task not found → 404.
- Permission denied → 403, print required capability.
