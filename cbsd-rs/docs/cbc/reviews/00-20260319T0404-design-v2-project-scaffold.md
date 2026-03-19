# Design Review: 00 — Project Scaffold (v2)

**Verdict: Approved.**

All v1 concerns resolved: cwd config fallback removed,
`put_json`/`put_empty` split, `url::Url` base URL, `dirs = "5"`,
`Error::Auth` collapsed into `Error::Api`.

No blockers. No major concerns.

## Minor Issues

- **`delete` return type convention undocumented.** Several
  server delete endpoints return `{"detail": "..."}`. The
  generic `delete<T: DeserializeOwned>` needs callers to
  specify `serde_json::Value` as `T`. Document the convention
  or add a `delete_detail` helper that returns the `detail`
  string directly.

- **`components` module absent from crate layout.** Doc 02
  references `GET /api/components/` but the scaffold's
  `src/` listing doesn't include a components module.

## Strengths

- `url::Url` for base URL prevents double-slash bugs.
- `put_json`/`put_empty` split is clean.
- `0600` config file permissions explicitly specified.
- `open` and `reqwest-eventsource` in dependency list.
