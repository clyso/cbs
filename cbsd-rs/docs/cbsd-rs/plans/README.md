# cbsd-rs Implementation Plans

## Overview

Phased implementation plan for the Rust rewrite of the CBS daemon (`cbsd/`).
The Rust workspace lives at `cbsd-rs/` in the repository root.

**Design documents:** `_docs/cbsd-rs/design/`

## Implementation Status

| Phase | Description | Commits | Status |
|-------|-------------|---------|--------|
| [Phase 0](phase-0-scaffolding.md) | Repository scaffolding and CLAUDE.md | 1 | Done |
| [Phase 1](phase-1-foundation.md) | Workspace, shared types, schema, server scaffold | 2 | Done |
| [Phase 2](phase-2-authentication.md) | PASETO, OAuth, API keys, extractors | 2 | Done |
| [Phase 3](phase-3-permissions-builds.md) | RBAC, build queue, submission, listing | 2 | Done |
| [Phase 4](phase-4-dispatch-logs.md) | WebSocket handler, dispatch, log writer, SSE | 4 | Done |
| [Phase 5](phase-5-worker.md) | WS client, build executor, subprocess bridge | 2 | Done |
| [Phase 6](phase-6-integration.md) | Startup recovery, bootstrapping, shutdown, GC | 2 | Done |
| [Phase 7](phase-7-worker-registration.md) | Worker registration, token-based identity, managed lifecycle | 5 | Done |
| [Phase 8](phase-8-sqlx-macros.md) | Compile-time checked SQL queries (sqlx macros) | 1 | Done |
| [Phase 9](phase-9-cbscore-wrapper.md) | cbscore wrapper — Python subprocess bridge for build execution | 1 | Done |
| [Phase 10](phase-10-periodic-builds.md) | Periodic build scheduling (cron, retry, CRUD API) | 1 | Done |

**Total:** 23 commits across 11 phases.

## Dependency Graph

```
Phase 0 → Phase 1 → Phase 2 → Phase 3 → Phase 4 → Phase 6
                                              ↓
                                          Phase 5 → Phase 6
                                                       ↓
                                                   Phase 7
```

Phase 5 (worker) depends on Phase 4 (server WS handler). Phase 6
(integration) depends on both Phases 4 and 5. Phase 7 (worker registration)
depends on Phase 6. Phase 8 (sqlx macros) is independent — applies at any
point after Phase 1. Phase 9 (cbscore wrapper) depends on Phase 5 (worker
executor). Phase 10 (periodic builds) depends on Phase 3 (build submission).

## Deferred (post-v1)

- Component management API
- Multiple concurrent builds per worker
- Build artifact tracking

## Conventions

- **Commit style:** Ceph project conventions
- **Sign-off:** `-s` flag, no GPG signing (`-c commit.gpgsign=false`)
- **No Co-Authored-By lines**
- **Each commit must compile** (`cargo build --workspace`) and pass tests
- **Update plan progress tables** after each commit lands
