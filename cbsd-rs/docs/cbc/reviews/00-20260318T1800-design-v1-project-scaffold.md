# Design Review: 00 — Project Scaffold

**Document:** `docs/cbc/design/00-20260318T1800-project-scaffold.md`

---

## Summary

The scaffold design is structurally sound. Crate layout,
dependencies, error types, and the HTTP client wrapper are all
appropriate for a Rust CLI. Two issues need fixing: the config
file resolution order has a credential-exposure hazard, and the
`put` method signature needs splitting.

**Verdict: Approve with conditions.**

---

## Blockers

None.

---

## Major Concerns

### M1 — `./cbc-config.json` fallback is a credential exposure risk

The resolution order puts `./cbc-config.json` (current directory)
as fallback position 3. If a user is in a shared directory that
happens to contain a `cbc-config.json` left by someone else, they
silently load a foreign token. The XDG path is the right default;
the current-directory fallback should be removed or demoted to
an explicit opt-in via `-c`.

**Fix:** Remove the cwd fallback from the resolution order.
Keep only: (1) `-c <path>`, (2) `$XDG_CONFIG_HOME/cbc/config.json`.

---

## Minor Issues

- **`put` signature with `Option<&impl Serialize>` is awkward.**
  `periodic enable`/`disable` send no body. Every call site must
  pass `None::<&()>`. Split into `put_json()` (with body) and
  `put_empty()` (no body).

- **`Error::Auth(String)` duplicates `Error::Api` for 401/403.**
  Consider collapsing `Auth` into `Api` and letting callers
  pattern-match on `status`.

- **`base_url: String` with manual `/api` concatenation is
  fragile.** Use `url::Url` and `url.join()` to prevent
  double-slash bugs.

- **`dirs = "6"` — verify version.** The latest published `dirs`
  is 5.x as of early 2026. Confirm before writing Cargo.toml.

- **No `--json` flag for machine-readable output.** If downstream
  automation is expected (CI pipelines), a JSON output mode should
  be designed in from the start.

---

## Strengths

- `rustls` with system root certs avoids OpenSSL dependency hell.
- `Authorization: Bearer` as default header keeps call sites clean.
- `0600` file permissions on config explicitly specified.
- `unauthenticated()` constructor cleanly handles login flow.
- `eprintln!` for debug is correct for a CLI (not `tracing`).

---

## Open Questions

1. Is `dirs = "6"` a typo for `"5"`?
2. Is a `--json` output mode planned for scripting?
