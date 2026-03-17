# Phase 8: Compile-Time Checked SQL Queries

**Design document:** `_docs/cbsd-rs/design/2026-03-17-sqlx-compile-time-queries.md`

## Progress

| # | Commit | Queries | ~LOC | Status |
|---|--------|---------|------|--------|
| 1 | `cbsd-rs/server: migrate to sqlx compile-time checked query macros` | 76 | ~1200 | Done |

**Total:** 76 queries migrated to `query!()` / `query_as!()`. 1 dynamic
query in `db/builds.rs` (`list_builds`) stays as `sqlx::query()`.

---

## Why one commit

The `query!()` macro requires either `DATABASE_URL` (live database at
compile time) or `SQLX_OFFLINE=true` with a committed `.sqlx/` cache.
Any commit introducing a `query!()` call without the `.sqlx/` cache
breaks the build for anyone without `DATABASE_URL`. Splitting the
migration across multiple commits would create a window where
intermediate commits only build under special conditions — violating
the rule that every commit in the history must compile and work.

The single commit enables the `macros` feature, migrates all 76 queries,
and commits the `.sqlx/` cache atomically. ~1200 LOC exceeds the
400–800 guideline, but the changes are mechanical (find-and-replace
pattern) and the alternative (broken intermediate commits) is worse.

---

## Prerequisites

```bash
export DATABASE_URL=sqlite:///tmp/cbsd-dev.db
cargo sqlx database create
cargo sqlx migrate run --source cbsd-rs/migrations/
```

This ephemeral database is needed only by the developer implementing
the migration (the `query!()` macro validates against it at compile
time). After the `.sqlx/` cache is committed, everyone builds with
`SQLX_OFFLINE=true` — no database needed.

---

## Commit 1: `cbsd-rs/server: migrate to sqlx compile-time checked query macros`

**Files:**
- `cbsd-rs/cbsd-server/Cargo.toml` (add `"macros"` to sqlx features)
- `cbsd-rs/cbsd-server/src/db/tokens.rs` (4 queries)
- `cbsd-rs/cbsd-server/src/db/users.rs` (3 queries)
- `cbsd-rs/cbsd-server/src/db/api_keys.rs` (8 queries)
- `cbsd-rs/cbsd-server/src/db/workers.rs` (7 queries)
- `cbsd-rs/cbsd-server/src/db/builds.rs` (9 of 10 — 1 dynamic stays)
- `cbsd-rs/cbsd-server/src/db/roles.rs` (21 queries)
- `cbsd-rs/cbsd-server/src/db/seed.rs` (5 queries)
- `cbsd-rs/cbsd-server/src/queue/recovery.rs` (7 queries)
- `cbsd-rs/cbsd-server/src/routes/admin.rs` (6 queries)
- `cbsd-rs/cbsd-server/src/routes/permissions.rs` (2 queries)
- `cbsd-rs/cbsd-server/src/logs/writer.rs` (1 query)
- `cbsd-rs/cbsd-server/src/logs/sse.rs` (1 query)
- `cbsd-rs/cbsd-server/src/logs/gc.rs` (2 queries)
- `cbsd-rs/.sqlx/` (new — offline query cache)

**Content:**

### Cargo.toml

```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "macros"] }
```

### Migration pattern for DML queries

```rust
// Before:
sqlx::query("INSERT INTO foo (a, b) VALUES (?, ?)")
    .bind(a).bind(b)
    .execute(pool).await?;

// After:
sqlx::query!("INSERT INTO foo (a, b) VALUES (?, ?)", a, b)
    .execute(pool).await?;
```

### Migration pattern for SELECT queries

Prefer `query_as!` when returning an existing named struct — eliminates
`row_to_*()` helpers and manual `Row::get()` mapping:

```rust
// Before:
let row = sqlx::query("SELECT email, name, active FROM users WHERE email = ?")
    .bind(email).fetch_optional(pool).await?;
Ok(row.map(|r| UserRecord {
    email: r.get("email"),
    name: r.get("name"),
    active: r.get::<i32, _>("active") != 0,
}))

// After:
let row = sqlx::query!("SELECT email, name, active FROM users WHERE email = ?", email)
    .fetch_optional(pool).await?;
Ok(row.map(|r| UserRecord {
    email: r.email,
    name: r.name,
    active: r.active != 0,
}))
```

Use `query!()` (anonymous struct) for ad-hoc queries like COUNTs and
EXISTS checks.

### Transaction queries

Same macro, pass `&mut *tx` as executor:

```rust
sqlx::query!("INSERT INTO api_keys (...) VALUES (?, ?, ?, ?)",
    name, hash, prefix, email)
    .execute(&mut *tx).await?;
```

### Exception: 1 dynamic query

`db/builds.rs` `list_builds` constructs SQL at runtime with optional
WHERE clauses. This stays as `sqlx::query(...)`.

### Offline cache

After all queries compile against `DATABASE_URL`:

```bash
cargo sqlx prepare --workspace
git add .sqlx/
```

Verify: `SQLX_OFFLINE=true cargo build --workspace`

---

## Notes

- **Mechanical changes only.** No logic changes, no new features, no
  behavioral differences. Every query does exactly what it did before —
  just validated at compile time instead of runtime.
- **After this commit,** changing a query or migration requires
  `DATABASE_URL` + `cargo sqlx prepare --workspace` to regenerate the
  cache. Building the project only requires `SQLX_OFFLINE=true`.
- **`query_as!` preferred** for SELECTs mapping to named structs
  (`BuildRecord`, `WorkerRow`, `ApiKeyRow`, etc.). `query!()` for
  ad-hoc results.
