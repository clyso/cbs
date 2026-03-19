# 00 — Project Scaffold

## Overview

New `cbc` crate in the `cbsd-rs` workspace. A Rust CLI client for
the cbsd-rs REST API, replacing the Python `cbc` package.

## Crate setup

```
cbsd-rs/
  cbc/
    Cargo.toml
    src/
      main.rs       # entry point, clap CLI
      config.rs     # config file loading + token storage
      client.rs     # reqwest HTTP client wrapper
      error.rs      # error types
```

Add to `cbsd-rs/Cargo.toml` workspace members:

```toml
[workspace]
members = [
    "cbsd-proto", "cbsd-server", "cbsd-worker", "cbc",
]
```

### Dependencies

```toml
[dependencies]
cbsd-proto = { path = "../cbsd-proto" }
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", features = [
    "json", "rustls-tls",
] }
serde.workspace = true
serde_json.workspace = true
tokio = { version = "1", features = ["full"] }
chrono.workspace = true
url = "2"                # URL parsing (base_url handling)
dirs = "5"               # XDG directory resolution
open = "5"               # open URLs in default browser
reqwest-eventsource = "0.6"  # SSE client (build logs)
```

`cbsd-proto` is shared with the server — provides
`BuildDescriptor`, `Priority`, `Arch`, `WorkerToken`, etc.

## CLI framework

Top-level `clap` derive CLI:

```rust
#[derive(Parser)]
#[command(
    name = "cbc",
    version,
    about = "CBS build service client",
)]
struct Cli {
    /// Config file path (overrides default location)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Enable debug output
    #[arg(short, long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Commands,
}

enum Commands {
    /// Authenticate with a CBS server
    Login(LoginArgs),
    /// Show current user identity and roles
    Whoami,
    /// Build operations
    Build(BuildArgs),
    /// Periodic build management
    Periodic(PeriodicArgs),
    /// Worker administration
    Worker(WorkerArgs),
    /// RBAC administration
    Admin(AdminArgs),
}
```

`login` and `whoami` are top-level (frequent use). `build`,
`periodic`, `worker`, `admin` are subcommand groups.

## Config file

### Resolution order

1. `-c <path>` CLI flag (highest priority).
2. `$XDG_CONFIG_HOME/cbc/config.json`
   (typically `~/.config/cbc/config.json`).

No current-directory fallback — loading a config from a
shared directory is a credential exposure risk. Users who
need a project-local config use `-c ./cbc-config.json`
explicitly.

If no config found and the command requires auth, exit
with: `"no config found — run 'cbc login <url>' first"`.

Use `dirs::config_dir()` for XDG resolution.

### Format

```json
{
  "host": "https://cbs.example.com",
  "token": "base64-encoded-paseto-token"
}
```

Minimal — just the server URL and the auth token. No
nested objects, no token metadata. The token is opaque to
the client; the server validates it.

### Storage

On `cbc login`, the config file is written to the XDG
location (`~/.config/cbc/config.json`). The directory is
created if absent (`mkdir -p` equivalent).

File permissions: `0600` (owner read/write only) on Unix.

### Loading

```rust
pub struct Config {
    pub host: String,
    pub token: String,
}

impl Config {
    pub fn load(
        path: Option<&Path>,
    ) -> Result<Self, ConfigError>;

    pub fn save(
        &self, path: &Path,
    ) -> Result<(), ConfigError>;
}
```

`load` follows the resolution order above. `save` writes
JSON with `0600` permissions.

## HTTP client wrapper

```rust
pub struct CbcClient {
    inner: reqwest::Client,
    base_url: url::Url,   // parsed, e.g. ".../api/"
    token: Option<String>,
}
```

`base_url` is a parsed `url::Url` (not a raw `String`).
Paths are joined with `url.join()` to prevent
double-slash bugs.

### Construction

```rust
impl CbcClient {
    pub fn new(
        host: &str, token: &str,
    ) -> Result<Self, Error>;

    /// For login and health (no token yet).
    pub fn unauthenticated(
        host: &str,
    ) -> Result<Self, Error>;
}
```

`new` appends `/api/` to the host, sets
`Authorization: Bearer <token>` as default header.
Configures TLS (rustls, system root certs).

### Methods

```rust
pub async fn get<T: DeserializeOwned>(
    &self, path: &str,
) -> Result<T, Error>;

pub async fn post<B: Serialize, T: DeserializeOwned>(
    &self, path: &str, body: &B,
) -> Result<T, Error>;

pub async fn put_json<T: DeserializeOwned>(
    &self, path: &str, body: &impl Serialize,
) -> Result<T, Error>;

pub async fn put_empty<T: DeserializeOwned>(
    &self, path: &str,
) -> Result<T, Error>;

pub async fn delete<T: DeserializeOwned>(
    &self, path: &str,
) -> Result<T, Error>;

pub async fn get_stream(
    &self, path: &str,
) -> Result<reqwest::Response, Error>;

/// Build a GET request (for SSE EventSource).
pub fn get_request(
    &self, path: &str,
) -> reqwest::RequestBuilder;
```

`put` is split into `put_json` (with body) and
`put_empty` (no body) — avoids `None::<&()>` at every
call site for enable/disable endpoints.

All methods check the HTTP status code. On 4xx/5xx,
parse the error body (`{"error": "message"}`) and return
a typed error.

## Error types

```rust
pub enum Error {
    /// Config file not found or invalid.
    Config(String),
    /// HTTP connection failure.
    Connection(String),
    /// Server returned an error response.
    Api { status: u16, message: String },
    /// Unexpected error.
    Other(String),
}
```

`Auth` is not a separate variant — 401/403 are handled
by `Api { status: 401, .. }`. Callers pattern-match on
`status` when they need auth-specific behavior (e.g.,
printing "session expired").

All errors implement `Display` and are printed to stderr.
The process exits with code 1 on any error.

## Debug output

When `--debug` is set:

- Print the full request URL and method before each
  request.
- Print the response status code.
- Print the response body on error.

Use `eprintln!` for debug output (not `tracing` — this
is a CLI tool, not a long-running service).
