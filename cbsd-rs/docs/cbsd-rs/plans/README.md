# cbsd-rs Implementation Plans

## Overview

Phased implementation plan for the Rust rewrite of the CBS daemon (`cbsd/`). The
Rust workspace lives at `cbsd-rs/` in the repository root.

**Design documents:** `cbsd-rs/docs/cbsd-rs/design/`

## Implementation Status

| Phase                                                  | Description                                                         | Commits | Status  |
| ------------------------------------------------------ | ------------------------------------------------------------------- | ------- | ------- |
| [Phase 0](001-20260318T1411-scaffolding.md)            | Repository scaffolding and CLAUDE.md                                | 1       | Done    |
| [Phase 1](002-20260318T1411-01-foundation.md)          | Workspace, shared types, schema, server scaffold                    | 2       | Done    |
| [Phase 2](003-20260318T1411-authentication.md)         | PASETO, OAuth, API keys, extractors                                 | 2       | Done    |
| [Phase 3](002-20260318T1411-02-permissions-builds.md)  | RBAC, build queue, submission, listing                              | 2       | Done    |
| [Phase 4](002-20260318T1411-03-dispatch-logs.md)       | WebSocket handler, dispatch, log writer, SSE                        | 4       | Done    |
| [Phase 5](002-20260318T1411-04-worker.md)              | WS client, build executor, subprocess bridge                        | 2       | Done    |
| [Phase 6](002-20260318T1411-05-integration.md)         | Startup recovery, bootstrapping, shutdown, GC                       | 2       | Done    |
| [Phase 7](004-20260316T1018-01-worker-registration.md) | Worker registration, token-based identity, managed lifecycle        | 5       | Done    |
| [Phase 7.5](004-20260316T1939-02-dev-mode-seeding.md)  | Dev mode worker seeding for podman-compose                          | 1       | Done    |
| [Phase 8](006-20260317T1028-sqlx-macros.md)            | Compile-time checked SQL queries (sqlx macros)                      | 1       | Done    |
| [Phase 9](007-20260318T0725-cbscore-wrapper.md)        | cbscore wrapper — Python subprocess bridge                          | 1       | Done    |
| [Phase 10](008-20260318T1713-periodic-builds.md)       | Periodic build scheduling (cron, retry, CRUD API)                   | 1       | Done    |
| [Phase 11](009-20260320T0800-dev-oauth-bypass.md)      | Dev mode OAuth bypass                                               | 1       | Done    |
| [Phase 12](016-20260402T1600-role-level-scopes.md)     | Role-level scopes                                                   | 4       | Pending |
| [Phase 13](017-20260419T2123-robot-accounts.md)        | Robot accounts (preparatory P1/P2/P3 + robot identity, tokens, CLI) | 8       | Pending |

**Total:** 37 commits across 15 phases.

## Dependency Graph

```
Phase 0 → Phase 1 → Phase 2 → Phase 3 → Phase 4 → Phase 6
                        ↓                     ↓
                   Phase 11              Phase 5 → Phase 6
                                                      ↓
                                                  Phase 7
                                                      ↓
                                                Phase 7.5
```

Phase 5 (worker) depends on Phase 4 (server WS handler). Phase 6 (integration)
depends on both Phases 4 and 5. Phase 7 (worker registration) depends on
Phase 6. Phase 7.5 (dev mode seeding) depends on Phase 7. Phase 8 (sqlx macros)
is independent — applies at any point after Phase 1. Phase 9 (cbscore wrapper)
depends on Phase 5 (worker executor). Phase 10 (periodic builds) depends on
Phase 3 (build submission). Phase 11 (dev OAuth bypass) depends on Phase 2
(authentication).

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
