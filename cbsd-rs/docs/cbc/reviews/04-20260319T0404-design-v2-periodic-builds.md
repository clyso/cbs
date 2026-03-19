# Design Review: 04 — Periodic Builds (v2)

**Verdict: Approve with conditions.**

Both v1 blockers resolved: `cron_expr`/`tag_format` field
names correct, dual capability requirement documented, enable
response limitation noted, `{base_tag}` defined.

No blockers.

## Major Concerns

### M1 — Descriptor construction step not explicit

The design says `"descriptor"` is "a raw JSON object
(serialized `BuildDescriptor`)" but doesn't specify that the
client must construct a full `BuildDescriptor` struct from
the flattened CLI args and then serialize it to
`serde_json::Value` before embedding in the request body. A
developer could mistakenly pass CLI args as a flat object.

**Fix:** Add a sentence: "The client constructs a
`BuildDescriptor` struct from the CLI options (same as
`build new`), serializes it to `serde_json::Value`, and
embeds it as the `descriptor` field in the POST body."

## Minor Issues

- **`summary` cannot be cleared via update.** The server
  does a null-preserving merge — once set, `summary` cannot
  be removed. Note this as a known server limitation.

- **`periodic new` creates tasks always enabled.** Consider
  documenting the option to disable immediately after
  creation for pre-staging.

## Strengths

- `cron_expr` and `tag_format` field names documented.
- Dual capability requirement (`periodic:create` +
  `builds:create`) with scope validation.
- `{base_tag}` defined as `--image-tag` value.
- Timestamps documented as Unix epoch with client conversion.
- `enable` response limitation honestly noted.
- Tag format variables in `--help` is good operator UX.
