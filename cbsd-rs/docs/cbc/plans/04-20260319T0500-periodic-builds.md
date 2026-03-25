# Plan 04: Periodic Builds

**Design document:**
`docs/cbc/design/04-20260318T1804-periodic-builds.md`

## Progress

| # | Commit | ~LOC | Status |
|---|--------|------|--------|
| 1 | `cbc: add periodic build commands` | ~450 | TODO |

## Why one commit

The seven periodic commands (`new`, `list`, `get`, `update`,
`delete`, `enable`, `disable`) share:

- The `Periodic` subcommand group and its `PeriodicCommands`
  enum.
- The same `BuildDescriptorArgs` from plan 02 (flattened
  into `periodic new` and `periodic update`).
- The same `CbcClient` methods from plan 00.

Five of the seven commands are simple one-endpoint calls
(list, get, delete, enable, disable) at 20-40 lines each.
`new` and `update` are larger (~80-100 lines) due to
descriptor construction and the periodic-specific fields
(`--cron`, `--tag-format`, `--summary`, `--priority`).

Splitting would over-fragment: the simple commands are too
small to stand alone, and `new`/`update` depend on the same
subcommand group and imports. At ~450 LOC the single commit
fits comfortably in the 400-800 target.

---

## Commit 1: `cbc: add periodic build commands`

Adds the `periodic` subcommand group with seven commands:
`new`, `list`, `get`, `update`, `delete`, `enable`,
`disable`.

### Files

```
cbsd-rs/cbc/src/main.rs       (add Periodic variant)
cbsd-rs/cbc/src/periodic.rs   (new)
```

### Content

#### `main.rs` changes

Add `Periodic(PeriodicArgs)` variant to the `Commands`
enum. Add dispatch arm calling `periodic::run()`.

#### `cbc/src/periodic.rs`

**Subcommand structure:**

```rust
#[derive(Args)]
pub struct PeriodicArgs {
    #[command(subcommand)]
    command: PeriodicCommands,
}

enum PeriodicCommands {
    New(PeriodicNewArgs),
    List,
    Get(PeriodicGetArgs),
    Update(PeriodicUpdateArgs),
    Delete(PeriodicDeleteArgs),
    Enable(PeriodicEnableArgs),
    Disable(PeriodicDisableArgs),
}
```

**`periodic new` args:**

```rust
#[derive(Args)]
struct PeriodicNewArgs {
    #[arg(long)]
    cron: String,
    #[arg(long)]
    tag_format: String,
    #[arg(long)]
    summary: Option<String>,
    #[command(flatten)]
    descriptor: BuildDescriptorArgs,
}
```

Flattens `BuildDescriptorArgs` from `builds.rs` (already
`pub` per plan 02). `--priority` comes from
`BuildDescriptorArgs`.

**`periodic new` flow:**

1. Load config (require auth).
2. Fetch `GET /api/auth/whoami` to get `name` and `email`
   for `signed_off_by` (same as `build new`).
3. Construct `BuildDescriptor` from the flattened
   `BuildDescriptorArgs` using the same parsing logic as
   `build new` (component `@` split, repo-override `=`
   split, version type/priority parsing).
4. Serialize descriptor to `serde_json::Value`.
5. Build request body:


   ```json
   {
     "cron_expr": "<cron>",
     "tag_format": "<tag_format>",
     "descriptor": { ... },
     "priority": "<priority>",
     "summary": "<summary>"
   }

   ```

6. `POST /api/periodic`.
7. Print task ID and state from response. Include
   schedule and next run time (convert epoch to UTC).

**`periodic list` flow:**

1. Load config. `GET /api/periodic`.
2. Print tabular output: ID (truncated 8 hex), enabled
   (yes/no), schedule, next run. Next run is `-` when
   disabled. Timestamps converted from Unix epoch.

**`periodic get` flow:**

1. Load config. `GET /api/periodic/{id}`.
2. Print aligned key-value detail view: id, cron,
   tag format, enabled, created by, next run, retries,
   last error, last build (id + trigger time).
3. Print descriptor section: version, channel, type,
   image, components, distro, priority, summary.

**`periodic update` flow:**

1. Load config.
2. All options are optional (at least one required).
   Same option set as `new` but all fields are
   `Option<T>`. Uses a separate `PeriodicUpdateArgs`
   struct (not `BuildDescriptorArgs`, since all fields
   must be optional). Only provided fields are included
   in the request body.
3. `PUT /api/periodic/{id}`.
4. Print `"periodic task {id} updated"`.

**`periodic delete` flow:**

1. Load config. `DELETE /api/periodic/{id}`.
2. Print `"periodic task {id} deleted"`.

**`periodic enable` flow:**

1. Load config. `PUT /api/periodic/{id}/enable`.
2. Print `"periodic task {id} enabled"`.

**`periodic disable` flow:**

1. Load config. `PUT /api/periodic/{id}/disable`.
2. Print `"periodic task {id} disabled"`.

### LOC estimate

| Component | ~Lines |
|-----------|--------|
| Subcommand enum + dispatch | ~50 |
| `PeriodicNewArgs` + `PeriodicUpdateArgs` | ~60 |
| `periodic new` (whoami + construct + post) | ~100 |
| `periodic update` (optional fields + put) | ~80 |
| `periodic list` (query + table format) | ~60 |
| `periodic get` (fetch + detail display) | ~60 |
| `periodic delete/enable/disable` | ~40 |
| **Total** | **~450** |

### Verification

```bash
cargo build --workspace
cargo clippy --workspace
cargo fmt --check
# Manual: cbc periodic --help
# Manual: cbc periodic new --help
# Manual: cbc periodic list
```
