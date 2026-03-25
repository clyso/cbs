# cbsd-rs Documentation

## Guides

- [UI Integration](ui-integration.md) — how the web UI SPA integrates with
  cbsd-rs for authentication, session management, and API access. Written for
  frontend engineers.
- [RBAC](rbac.md) — roles, capabilities, scopes, and the `cbc` CLI commands for
  managing permissions. Written for operators and developers working on the
  permission system.

## Design, Plan, and Review Documents

Design documents, implementation plans, and review records live under
package-specific directories:

```
docs/
├── cbc/
│   ├── design/       # cbc feature designs
│   ├── plans/        # cbc implementation plans
│   └── reviews/      # cbc design, plan, and impl reviews
└── cbsd-rs/
    ├── design/       # cbsd-rs feature designs
    ├── plans/        # cbsd-rs implementation plans
    └── reviews/      # cbsd-rs design, plan, and impl reviews
```

**Routing:** `cbc` package documents go under `docs/cbc/`. All cbsd-rs workspace
packages (`cbsd-proto`, `cbsd-server`, `cbsd-worker`) go under `docs/cbsd-rs/`.

**Naming convention:** files follow a `<seq>-<timestamp>-

<title>.md` pattern. See the `cbsd-rs-docs` skill for the
full naming specification, sequence numbering rules, and
timestamp format.

These directories are not indexed here — browse them directly or use the
sequence numbers to trace related documents across design, plan, and review
directories.
