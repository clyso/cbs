# Design Review: 04 — Periodic Builds (v3)

**Verdict: Approved.**

The v2 concern is resolved: the descriptor construction step
is explicitly documented ("constructs a `BuildDescriptor`
struct from the CLI options, serializes it to
`serde_json::Value`, and embeds it as the `descriptor` field
in the POST body").

No blockers. No major concerns.

## Minor Issues

- `summary` cannot be cleared via update (server limitation
  from null-preserving merge). Not documented but is a minor
  operational detail.

## Strengths

- Descriptor construction step is now explicit.
- Field names (`cron_expr`, `tag_format`) correct.
- Dual capability requirement documented.
- `enable` response limitation honestly noted.
- `{base_tag}` defined.
- `--tag-format` included in update options.
