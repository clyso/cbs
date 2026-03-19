# Design Review: 04 — Periodic Builds

**Document:** `docs/cbc/design/04-20260318T1804-periodic-builds.md`

---

## Summary

Two blockers: the POST body field names don't match the server
(`cron` vs `cron_expr`), and the dual capability requirement
(`periodic:create` + `builds:create`) is undocumented. The
`enable` response format is wrong — it doesn't include
`next_run`.

**Verdict: Revise and re-review.**

---

## Blockers

### B1 — Field name mismatch in POST body

The server's `CreateTaskBody` uses `cron_expr`, not `cron`.
Similarly `tag_format`, not `tag`. The CLI option `--cron`
must serialize to `{"cron_expr": "..."}`.

The `descriptor` field must be `serde_json::Value` (raw JSON
object), not a named `BuildDescriptor` type in the POST body.

**Fix:** Document the exact JSON field names. Ensure the clap
options serialize to matching names (or use `#[serde(rename)]`).

### B2 — Dual capability requirement undocumented

The server requires both `periodic:create` AND `builds:create`
for `POST /api/periodic`. A user with `periodic:create` but
not `builds:create` gets 403. The design doesn't mention this.

Similarly, `PUT /api/periodic/{id}` requires `builds:create`
when updating the descriptor.

**Fix:** Add "Requires" annotations: `periodic:create` +
`builds:create` (scoped) for create, `periodic:manage` +
`builds:create` (scoped, if descriptor changed) for update.

---

## Major Concerns

### M1 — `enable` response does not contain `next_run`

The design shows:

```
periodic task 550e8400-... enabled
  next run: 2026-03-19 02:00:00 UTC
```

The server's enable handler returns only
`{"detail": "periodic task '...' enabled"}`. No `next_run`.
To show it, the client must follow up with
`GET /api/periodic/{id}`.

**Fix:** Either drop `next_run` from the enable output or
document the two-request pattern.

### M2 — `{base_tag}` variable is unexplained

The variable table includes `{base_tag}` without definition.
It resolves to `descriptor.dst_image.tag` — the `--image-tag`
value. Operators won't know this without reading server code.

**Fix:** Add: "`{base_tag}` — the value of `--image-tag`."

### M3 — Timestamps in response are Unix epoch integers

All `PeriodicTaskResponse` timestamps (`created_at`, `next_run`,
`last_triggered_at`, etc.) are `i64` epoch seconds. The design
shows human-readable times. The client must convert. The
`last build: #42 at 2026-03-18 02:00` display requires
combining `last_build_id` with `last_triggered_at` (trigger
time, not build completion time).

**Fix:** Document the conversion and clarify which timestamp
is used for the "at" display.

---

## Minor Issues

- **No `--disabled` flag on `periodic new`.** Tasks are always
  created enabled. Consider allowing creation in disabled state
  for pre-staging.

- **`periodic list` UUID truncation length unspecified.** Define
  a standard (e.g., 8 hex chars).

- **`periodic update` missing `--tag-format` in options table.**
  It should be listed since it's an updatable field.

- **`priority` field is a raw string on the server.** Client
  should validate `high`/`normal`/`low` before sending.

- **`periodic delete` doesn't mention in-progress builds.**
  Deleting a task does not cancel builds already triggered.

---

## Strengths

- `#[command(flatten)]` for shared `BuildDescriptorArgs` is
  the correct Rust idiom.
- Tag format variable table in `--help` is good operator UX.
- `enable` resets retry state — correctly documented.
- Endpoint mapping is accurate for all 7 routes.
