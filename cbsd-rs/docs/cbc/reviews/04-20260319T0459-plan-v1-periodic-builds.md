# Plan Review: 04 — Periodic Builds

**Verdict: Approved.**

Single commit at ~450 LOC — within the 400-800 target. Seven
commands share the subcommand group and `BuildDescriptorArgs`.

The plan faithfully tracks the design:


- `periodic new` flattens `BuildDescriptorArgs` from
  `builds.rs` (already `pub`).
- `--version` is `#[arg(long)]` (named flag, not positional).
- Descriptor construction: struct → `serde_json::Value` →
  embed in body. Explicitly documented.
- Request body uses `cron_expr` and `tag_format` field names.
- Dual capability (`periodic:create` + `builds:create`) noted.
- `periodic update` uses separate `PeriodicUpdateArgs` with
  all-optional fields (not `BuildDescriptorArgs`).
- `enable`/`disable` correctly omit `next_run` from output.
- UUID truncation to 8 hex chars in list.
- Timestamps converted from Unix epoch.

No blockers. No major concerns.

## Minor Issues

- **`--priority` comes from `BuildDescriptorArgs`.** The
  periodic-specific options table lists `--priority` under
  periodic options, but the plan says it comes from
  `BuildDescriptorArgs`. This is correct — the shared struct
  carries it. Just ensure it doesn't appear twice in
  `--help`.

## Cross-Plan Consistency

- Depends on `BuildDescriptorArgs` from plan 02's
  `builds.rs`. The import path
  `crate::builds::BuildDescriptorArgs` is clean.
- `periodic new` calls whoami (same as `build new`) to
  populate `signed_off_by`.
