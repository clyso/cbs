# Config Keys: snake_case to kebab-case

## Decision

All user-facing YAML config keys in cbsd-rs use kebab-case. Rust struct
fields remain snake_case — serde's `#[serde(rename_all = "kebab-case")]`
handles the translation at the deserialization boundary.

## Rationale

kebab-case is the conventional format for YAML configuration files (used
by Kubernetes, Docker Compose, Ceph, Grafana, and most infrastructure
tooling). snake_case was an artifact of serde's default behavior mapping
Rust field names directly to serialized keys.

## Scope

10 structs across 2 files gain `#[serde(rename_all = "kebab-case")]`:

- `cbsd-server/src/config.rs`: `ServerConfig`, `SecretsConfig`,
  `OAuthConfig`, `TimeoutsConfig`, `LogRetentionConfig`, `SeedConfig`,
  `DevConfig`, `DevSeedWorker`, `LoggingConfig`
- `cbsd-worker/src/config.rs`: `WorkerConfig`

Both example configs (`config/server.yaml.example`,
`config/worker.yaml.example`) are updated to match.

22 of 32 fields change (the 10 single-word fields like `name`, `arch`,
`level` are unaffected). Zero impact on Rust code — field access remains
`config.listen_addr`, `config.tls_cert_path`, etc.
