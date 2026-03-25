# Plan 00: Project Scaffold + Authentication

**Design documents:**


- `docs/cbc/design/00-20260318T1800-project-scaffold.md`
- `docs/cbc/design/01-20260318T1801-authentication.md`

## Rationale for combining

The scaffold (crate setup, CLI framework, config, HTTP client,
error types) produces no usable commands on its own. Without
`login` and `whoami`, every module is dead code — nothing
exercises the config save path, the HTTP client, or the error
display. Shipping scaffold alone would violate the "each commit
must be independently useful" rule.

Combined, the first commit delivers a usable state: a user can
run `cbc login <url>`, obtain a token, and verify with
`cbc whoami`.

## Progress

| # | Commit | ~LOC | Status |
|---|--------|------|--------|
| 1 | `cbc: add project scaffold with login and whoami` | ~550 | TODO |

## Commit 1: `cbc: add project scaffold with login and whoami`

New `cbc` crate in the workspace. Implements the full
scaffold (CLI, config, HTTP client, errors) plus the two
auth commands (`login`, `whoami`).

### Files

```
cbsd-rs/Cargo.toml              (add "cbc" to workspace members)
cbsd-rs/cbc/Cargo.toml          (new)
cbsd-rs/cbc/src/main.rs         (new)
cbsd-rs/cbc/src/config.rs       (new)
cbsd-rs/cbc/src/client.rs       (new)
cbsd-rs/cbc/src/error.rs        (new)
```

### Content

#### `Cargo.toml` (workspace root)

Add `"cbc"` to workspace members:

```toml
members = [
    "cbsd-proto", "cbsd-server", "cbsd-worker", "cbc",
]
```

#### `cbc/Cargo.toml`

```toml
[package]
name = "cbc"
version = "0.1.0"
edition.workspace = true
license = "GPL-3.0-or-later"

[dependencies]
cbsd-proto = { path = "../cbsd-proto" }
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", features = [
    "json", "rustls-tls",
] }
serde.workspace = true
serde_json.workspace = true
tokio = { version = "1", features = ["full"] }
url = "2"
dirs = "5"
open = "5"
```

No `reqwest-eventsource` yet — added in plan 03 when
build log following is implemented.

#### `cbc/src/error.rs`

```rust
pub enum Error {
    Config(String),
    Connection(String),
    Api { status: u16, message: String },
    Other(String),
}
```

Implements `Display` and `std::error::Error`. The
`Display` impl formats human-readable messages for
stderr. Process exits with code 1 on any error
(handled in `main.rs`).

#### `cbc/src/config.rs`

```rust
pub struct Config {
    pub host: String,
    pub token: String,
}
```

- `Config::load(path: Option<&Path>)` — resolution
  order: CLI flag, then `dirs::config_dir()/cbc/
  config.json`. Returns `Error::Config` if not found
  and command requires auth.
- `Config::save(&self, path: &Path)` — writes JSON,
  creates parent dirs, sets `0600` permissions on Unix
  via `std::fs::set_permissions`.
- `Config::default_path()` — returns the XDG path.

#### `cbc/src/client.rs`

```rust
pub struct CbcClient {
    inner: reqwest::Client,
    base_url: url::Url,
    token: Option<String>,
    debug: bool,
}
```

- `CbcClient::new(host, token, debug)` — parses host,
  appends `/api/`, sets Bearer header.
- `CbcClient::unauthenticated(host, debug)` — no
  token, for login health check and token validation.
- Generic methods: `get<T>`, `post<B, T>`, `put_json`,
  `put_empty`, `delete<T>`, `get_stream`,
  `get_request`.
- All methods check HTTP status. On 4xx/5xx, parse
  `{"error": "..."}` body into `Error::Api`.
- When `debug` is true, `eprintln!` the method, URL,
  and response status for each request.

#### `cbc/src/main.rs`

Top-level CLI with clap derive:

```rust
#[derive(Parser)]
#[command(name = "cbc", version)]
struct Cli {
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[arg(short, long, global = true)]
    debug: bool,
    #[command(subcommand)]
    command: Commands,
}

enum Commands {
    Login(LoginArgs),
    Whoami,
}
```

Only `Login` and `Whoami` variants for now. `Build`,
`Periodic`, `Worker`, `Admin` are added by later plans.

**`login` command flow:**

1. Validate server: `GET /api/health` via
   unauthenticated client. On failure:
   `"cannot reach server at <url>"`.
2. Construct login URL:
   `{url}/api/auth/login?client=cli`.
3. `open::that(&login_url)` — open in browser.
   Print the URL regardless (fallback for headless).
4. Prompt: `"Paste the token here: "` — read line
   from stdin.
5. Validate token: `GET /api/auth/whoami` with
   the pasted token.
6. Save config to XDG path.
7. Print: `"logged in as <email>"`.

**`whoami` command flow:**

1. Load config (require auth).
2. `GET /api/auth/whoami`.

3. Print aligned key-value output:

   ```
     email: admin@clyso.com
      name: Admin
     roles: admin

      caps: *
   ```

4. On 401: print `"session expired — run 'cbc login
   {host}' to re-authenticate"` using the stored
   host from config.

### LOC estimate

| Component | ~Lines |
|-----------|--------|
| `error.rs` | ~50 |
| `config.rs` | ~80 |
| `client.rs` | ~180 |
| `main.rs` (CLI + login + whoami) | ~180 |
| `Cargo.toml` (crate + workspace) | ~30 |
| **Total** | **~520** |

### Verification

```bash
cargo build --workspace
cargo clippy --workspace
cargo fmt --check
# Manual: cbc --help, cbc login <url>, cbc whoami
```
