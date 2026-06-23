# 004 — Configuration, secrets & Vault

This is the reference design for the config, secrets, and Vault subsystem of the
`cbscore` library: the on-disk config format, the secrets-file format and its
credential families, the `SecretsMgr` that resolves credentials (from file or
Vault), and the Vault client. It owns these types and behaviors as the single
source of truth. Read 001 for the schema policy and the "configuration is
hand-authored" decision, 002 for the casing/marker machinery and the
error-taxonomy split, and 003 for the subprocess/redaction layer the resolved
credentials feed into.

Source of truth: `cbscore/config.py`, `cbscore/utils/secrets/` (`models.py`,
`mgr.py`, the per-family `git`/`storage`/`signing`/ `registry` resolvers),
`cbscore/utils/vault.py`.

There is **no `config init`** (001): operators hand-author the config, secrets,
and vault YAML files, so this design and the cbscore-rs README together must
fully specify their formats.

## Owned, versioned formats

All three files here are cbscore-**owned**, so each carries a `schema_version`
marker (001/002). They are kebab-cased (002), so the marker key is
**`schema-version`**, and `absent → v1`:

- the main **config** (`Config`),
- the **vault config** (`VaultConfig`, a separate file),
- the **secrets** file (`Secrets`).

YAML is the primary on-disk form; load dispatches on file suffix (`.yaml` →
YAML, otherwise JSON), matching `Config.load` / `Secrets.load` /
`VaultConfig.load`. Store always writes YAML. (Python round-trips through
`model_dump_json` then `yaml.safe_dump` to coerce `Path`; the Rust port uses
`camino::Utf8PathBuf`, which serializes as a plain string, so no double
conversion is needed.)

## Configuration types

Source: `config.py`. All structs are `#[serde(rename_all = "kebab-case")]`,
which reproduces Python's per-field aliases exactly — Python aliases only the
multi-word fields (`scratch-containers`, `log-file`, `vault-addr`, `auth-user`,
`auth-approle`, `auth-token`, `role-id`, `secret-id`), and kebab-case leaves
single-word fields unchanged.

```rust
struct Config {
    schema_version: u32,                  // "schema-version"; absent → 1
    paths: PathsConfig,
    storage: Option<StorageConfig>,
    signing: Option<SigningConfig>,
    logging: Option<LoggingConfig>,
    #[serde(default)]
    secrets: Vec<Utf8PathBuf>,            // paths to secrets files
    vault: Option<Utf8PathBuf>,           // path to the vault config file
}

struct PathsConfig {
    components: Vec<Utf8PathBuf>,
    scratch: Utf8PathBuf,
    scratch_containers: Utf8PathBuf,      // "scratch-containers"
    #[serde(skip_serializing_if = "Option::is_none")]
    ccache: Option<Utf8PathBuf>,
    // versions: Option<Utf8PathBuf> — added in M5 (design 006); not built in M1
}

struct StorageConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    s3: Option<S3StorageConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registry: Option<RegistryStorageConfig>,
}
struct S3StorageConfig { url: String, artifacts: S3LocationConfig, releases: S3LocationConfig }
struct S3LocationConfig { bucket: String, loc: String }
struct RegistryStorageConfig { url: String }   // FIXME-noted as ignored in Python

struct SigningConfig {                    // both optional; secret IDs
    #[serde(skip_serializing_if = "Option::is_none")]
    gpg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transit: Option<String>,
}

struct LoggingConfig { log_file: Utf8PathBuf }   // "log-file"
```

`storage`, `signing`, `logging` are optional at the top level; the runner and
builder degrade gracefully when they are absent (e.g. "not uploading" when
`storage.s3` is `None`), matching the Python guards. The serialize-shape of
optional fields follows 002's rule (emit nothing for a `None` that Python omits;
emit the value otherwise) — Python uses field-level defaults here, so `None`
optionals are simply absent.

`Config` exposes `load`/`store`, plus `get_secrets()` (merge the secrets files
in order — later files override earlier keys, matching `Secrets.merge`) and
`get_vault_config()` (load the vault file if `vault` is set).

## Vault configuration

Source: `config.py` (`VaultConfig`). A separate kebab-cased file.

```rust
struct VaultConfig {
    schema_version: u32,                  // "schema-version"; absent → 1
    vault_addr: String,                   // "vault-addr"
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_user: Option<VaultUserPass>,     // "auth-user"
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_approle: Option<VaultAppRole>,   // "auth-approle"
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_token: Option<String>,           // "auth-token"
}
struct VaultUserPass { username: String, password: String }
struct VaultAppRole { role_id: String, secret_id: String }   // role-id / secret-id
```

## Secrets file format

Source: `utils/secrets/models.py`. The `Secrets` container is four maps, keyed
by the resource they apply to:

```rust
struct Secrets {
    schema_version: u32,                  // "schema-version"; absent → 1
    #[serde(default)] git: BTreeMap<String, GitSecret>,       // key: repo URL/host
    #[serde(default)] storage: BTreeMap<String, StorageSecret>, // key: S3 URL
    #[serde(default)] sign: BTreeMap<String, SigningSecret>,    // key: secret id
    #[serde(default)] registry: BTreeMap<String, RegistrySecret>, // key: registry URL
}
```

`load`/`store`/`merge` mirror Python (`merge` = per-map key override).

### Credential families and discrimination

Every secret carries `creds: "plain" | "vault"`; a `vault` secret also carries
`key` (the Vault path to read). The Rust port models each family as a **single
enum with a custom `Deserialize`** that mirrors Python's per-family
discriminator function (`models.py`: `git_secret_discriminator`,
`storage_secret_discriminator`, `signing_secret_discriminator`,
`registry_secret_discriminator`) — each inspects the entry and returns the
variant tag. A single combined discriminator (rather than serde's nested
internally-tagged enums, which buffer into a content map and do not nest
cleanly) keeps the four families uniform and yields clear errors on an unknown
combination. The discriminating fields per family:

| Family   | Discriminators           | Variants (Python tags)                                                                                             |
| -------- | ------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| storage  | `creds` + `type=s3`      | `plain-s3`, `vault-s3`                                                                                             |
| signing  | `creds` + `type`         | `plain-gpg-key` (gpg-armor-key), `vault-gpg-single-key`, `vault-gpg-pvt-key`, `vault-gpg-pub-key`, `vault-transit` |
| registry | `creds` only             | `plain-registry`, `vault-registry`                                                                                 |
| git      | `creds` + `type` (added) | `plain-ssh`, `plain-token`, `plain-https`, `vault-ssh`, `vault-https`                                              |

Field shapes (kebab-cased; `vault` variants add `key`):

- **storage**: `{access-id, secret-id}` (+ `type: s3`).
- **signing** (each gpg variant is distinct): `gpg-armor-key` (plain)
  `{private-key, public-key?, passphrase?, email}`; `gpg-single-key` (vault)
  `{private-key, public-key?, passphrase?, email}`; `gpg-pvt-key` (vault)
  `{private-key, passphrase?, email}`; `gpg-pub-key` (vault)
  `{public-key, email}`; `transit` (vault) `{mount}`.
- **registry**: `{username, password, address}`.
- **git**: ssh `{ssh-key, username}`, token `{token, username}` (plain only),
  https `{username, password}`.

### Git secrets: an added `type` tag (chosen wire change)

This is the one operator-visible format change in 004, and it must be in the
README. Python discriminates **git** secrets by _field shape_ —
`git_secret_discriminator` (`models.py:89-126`) inspects the entry for `ssh-key`
/ `token` / `username+password` _within_ the `creds` tag. The custom
discriminator could replicate that shape inspection with no wire change, but the
port instead **adds an explicit `type: ssh | token | https` field to git
secrets** — a deliberate choice for a simpler, more robust discriminator and
consistency with the storage and signing families (which already carry `type`).
The valid `(creds, type)` combinations are exactly Python's five variants —
`token` is plain-only; there is no `vault-token`. Operators converting an
existing secrets file add one `type:` line to each git entry; the rest is
unchanged. (Storage, signing, and registry need no operator change —
`type`/`creds` already discriminate them.)

## Secrets manager

Source: `utils/secrets/mgr.py`. `SecretsMgr` wraps a parsed `Secrets` plus an
optional `Vault`, and resolves credentials on demand — from the plain entry, or
by reading the Vault `key` for `vault` entries. On construction with a vault
config it builds the client and verifies the connection
(`check_vault_connection`), surfacing `SecretsMgrError`.

Resolution surface. **Lookup strategy differs by family**: `storage` and
`signing`/`transit` look up by **exact** key/id (`secrets.get`), while `git` and
`registry` use **longest-prefix URI matching** (`find_best_secret_candidate` →
`matches_uri`; `utils.py:20-48`, `git.py:244`) — the secret whose key is the
closest-matching prefix of the URL wins. Each then resolves plain-or-vault (a
`vault` entry reads its `key` from Vault) and returns plaintext for immediate
use at a tool boundary:

- `s3_creds(url) -> (hostname, access_id, secret_id)` — exact key; consumed by
  the S3 client (005). _(first consumer: C4 build upload / C6 reuse)_
- `registry_creds(uri) -> (address, username, password)` — longest-prefix;
  consumed by buildah/skopeo push (003 wrappers, 008). _(C5)_
- `transit(id) -> (mount, key)` — exact id; Vault transit signing config. _(C6)_
- `git_url_for(url) -> Guard<CmdArg>` — longest-prefix; three cases: an **SSH**
  secret writes `~/.ssh/{config,known_hosts,id_<rnd>}` and yields an SSH
  **alias** `<alias>:<path>` (the temp key file is removed on guard drop); an
  **HTTPS** or **token** secret yields a `SecureUrl` with the credentials folded
  in (no temp files); **no match** yields the URL wrapped unchanged. _(C3)_
- `gpg_signing_key(id) -> Guard<(keyring_path, passphrase?, email)>` — exact id;
  materialises a temp GNUPGHOME keyring (key imported via
  `gpg --import --batch`), removed on guard drop. _(C4)_
- predicates: `has_vault`, `has_s3_creds`, `has_gpg_signing_key`,
  `has_transit_key`, `has_registry_creds`.

**Context managers → RAII guards.** Python exposes `git_url_for` and
`gpg_signing_key` as context managers because they may materialise temporary
resources (the SSH config/key files; a temp GPG keyring) that must be cleaned up
after use. The Rust port returns an RAII **guard** whose `Drop` removes that
material; the resolved value borrows from the guard, so it cannot outlive
cleanup. For the `git_url_for` HTTPS/token and no-match cases there is nothing
to clean up (the guard's `Drop` is a no-op). The other resolvers return owned
plaintext tuples directly. Resolved credentials are wrapped in
`Password`/`SecureUrl` (003) before they ever reach a command line.

## Vault client

Source: `utils/vault.py`. The client supports three auth backends and reads KV
v2 secrets.

- **Auth selection order: AppRole → userpass → token** (`vault.py:165-184`) —
  the first configured method wins. (This is the corrected order from the 000
  review's H5.)
- **KV v2 mount is `ces-kv`** (`vault.py:61`), pinned as a constant; every read
  passes it explicitly.
- `read_secret(path) -> map` reads `<ces-kv>/<path>` and returns the secret's
  `data.data`. Vault-backed secret entries resolve their value by reading their
  `key` through this.
- Errors map to `VaultError` (forbidden → permission-denied; others wrapped).
  `vaultrs` replaces Python's `hvac`.

Built from `VaultConfig` via the equivalent of `get_vault_from_config`.

## Operator initialization (README deliverable)

Because `config init` is not ported, the cbscore-rs README must document, with a
complete worked worker-deployment example: the `Config` fields (required vs
optional, the `schema-version`), the `VaultConfig` auth blocks, and every
secrets family — including the **new git `type:` field** and the `creds`/`type`
discriminators. This design specifies the formats; the README is their
operator-facing rendering.

## Fidelity notes

- **Git `type:` is added** (a chosen wire change, see above) — the only
  operator-visible format change; storage/signing/registry are unchanged.
- **Per-family lookup**: storage and signing/transit resolve by exact key/id;
  git and registry by longest-prefix URI match (`find_best_secret_candidate`).
- **`transit` returns `(mount, key)`** (`signing.py:184`), not `(key, mount)`.
- **`config init` dropped** (001) — no prompt/flag surface here.
- **YAML primary, JSON accepted by suffix**; store writes YAML.
- **Vault auth order** AppRole → userpass → token; **mount `ces-kv`**.
- **`schema-version`** (kebab) on config, vault config, and secrets;
  `absent → v1`.
- The `RegistryStorageConfig.url` is carried but ignored by the builder today
  (Python FIXME at `config.py:131-137`); kept for parity so config validation
  does not reject it.

## Errors

Per 002, these IO-layer errors live with this subsystem (in `cbscore`):
`ConfigError` (load/store/validation), `SecretsError` / `SecretsMgrError`
(secrets load/merge/resolution), `VaultError` (auth/read). They wrap their
sources with `thiserror`; no secret value is ever included in an error message
(the resolved values are `Password`/`SecureUrl`, 003).

## Testing

- **Round-trip** for `Config`, `VaultConfig`, `Secrets`
  (`serialize → parse → equal`), and golden-file parity against Python's YAML
  for representative files (accounting for the added `schema-version` and the
  git `type:`).
- **Discrimination**: each family parses to the right variant for every
  `(creds, type)` combination; an unknown/invalid combination errors clearly;
  the **git `type:`** tag selects the right variant and a shape-only (type-less)
  git entry is rejected with a helpful message.
- **Lookup strategy**: storage/signing resolve by exact key/id; git/registry
  resolve by longest-prefix URI match (the `find_best_secret_candidate` cases,
  including partial-match and tie-break by remainder depth).
- **Merge**: later secrets files override earlier keys per map.
- **Vault auth order**: with multiple methods configured, AppRole is chosen;
  with only a token, the token backend is used.
- **`absent → v1`** for all three markers; a higher marker is rejected.
- **No leak**: resolved credentials render redacted in any log/Debug path.
