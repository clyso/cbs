# Compile-Time Checked SQL Queries

## Problem

All 77 sqlx queries in cbsd-rs use `sqlx::query("...")` — the runtime
string API. These are never validated at compile time: a typo in a column
name, a missing table, or a type mismatch compiles successfully and fails
at runtime. The `.sqlx/` offline cache has never been committed because
without the `query!()` macro there is nothing to cache.

## Solution

Migrate from `sqlx::query("...")` to `sqlx::query!("...")` (and
`sqlx::query_as!` where appropriate). The macro connects to a live SQLite
database at compile time (via `DATABASE_URL`) and validates every query
against the actual schema. For CI and container builds without a database,
`SQLX_OFFLINE=true` reads from the committed `.sqlx/` cache.

## Scope

77 queries across 13 files in `cbsd-server`:

| File | Queries | Notes |
|---|---|---|
| `db/roles.rs` | 21 | Largest — roles, caps, assignments, scopes |
| `db/builds.rs` | 10 | 1 dynamic WHERE query (cannot use macro) |
| `db/api_keys.rs` | 8 | Includes transaction variants |
| `db/workers.rs` | 7 | Includes transaction variants |
| `queue/recovery.rs` | 7 | Startup recovery — SELECTs and UPDATEs |
| `db/seed.rs` | 5 | All inside transactions |
| `db/tokens.rs` | 4 | Simple CRUD |
| `db/users.rs` | 3 | Simple CRUD |
| `routes/admin.rs` | 6 | Inline SQL in handlers |
| `routes/permissions.rs` | 2 | Inline SQL in handlers |
| `logs/gc.rs` | 2 | Log garbage collection |
| `logs/writer.rs` | 1 | Log size update |
| `logs/sse.rs` | 1 | Log metadata lookup |

### What changes per query

**SELECT queries (40 queries):** Replace `sqlx::query("...").fetch_*()` +
manual `Row::get("col")` extraction with compile-time checked macros.
Prefer `sqlx::query_as!(ExistingStruct, "...")` when the query result
maps to an existing named struct (e.g., `BuildRecord`, `WorkerRow`) —
this eliminates the manual `row_to_*()` mapping code entirely. Use
`sqlx::query!("...")` (anonymous struct) for ad-hoc queries that don't
map to a named type.

**DML queries (37 queries):** Replace `sqlx::query("...").execute()` with
`sqlx::query!("...").execute()`. No struct change needed — the macro
validates parameters and column existence.

**Transaction queries (20 queries):** `sqlx::query!()` works with
transactions — the generated code is generic over `sqlx::Executor`.

### What cannot migrate

**1 query** in `db/builds.rs` (`list_builds`) constructs the SQL string
at runtime with optional WHERE clauses based on filter parameters. This
must remain `sqlx::query(...)` because the macro requires a string literal.
This is acceptable — it is a single query with a well-understood structure.

## Build requirements

### Development

```bash
export DATABASE_URL=sqlite:///tmp/cbsd-dev.db
cargo sqlx database create
cargo sqlx migrate run --source migrations/
cargo build --workspace  # query!() connects to DATABASE_URL
```

`DATABASE_URL` must point to a SQLite database with all migrations applied.
The macro verifies queries at compile time against this live database.

### CI / Container builds

```bash
SQLX_OFFLINE=true cargo build --workspace
```

Reads from the committed `.sqlx/` cache. No database needed.

### After changing queries or migrations

```bash
cargo sqlx prepare --workspace
git add .sqlx/
```

This regenerates the cache. It must be committed alongside the query
changes.

## Cargo.toml change

Add `"macros"` to the sqlx feature list:

```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "macros"] }
```

## Migration strategy

The migration is mechanical and can be done file by file. Each file is
an independent commit — the macro and string API coexist in the same
crate. Order doesn't matter for correctness; grouping by module keeps
commits reviewable.

The `.sqlx/` cache is generated once at the end and committed as the
final commit. Intermediate commits compile against a live `DATABASE_URL`.
