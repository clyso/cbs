# 001 — Implementation review: architecture & implementation spine (v5)

Adversarial implementation review of the **C4a slice** — the
credential/object-store infrastructure layer of the `cbscore` Rust port. This
continues the running implementation-review series (v1 architecture, v2 = C2
build, v3 = C3, v4 = C3 fix-round); v5 reviews C4a.

- **Target:** two commits on `wip/cbscore-rs`, range `d889389..HEAD`
  - `d52b50e` — cbscore: read git credentials from Vault (C4a) [C4a-1]
  - `f33c610` — cbscore: upload and download build artifacts over S3 (C4a)
    [C4a-2]
- **Designs (authoritative):** 004 (config/secrets/Vault), 005 (S3/releases),
  012 (static-musl/TLS).
- **Python source of truth:** `cbscore/utils/vault.py`,
  `cbscore/utils/secrets/{git,storage,mgr}.py`, `cbscore/utils/s3.py`.
- **Plan:** `plans/001-20260623T1725-03-build.md` (M2, commit C4a).

Method: every behavioral claim was verified by reading the Rust code, the Python
source, the design, and — for the two load-bearing third-party claims — the
vendored crate sources (`vaultrs-0.7.4`, `aws-smithy-http-client-1.1.13`). The
slice was built, linted, and tested from a clean state
(`cargo test -p cbscore --lib`: **106 passed, 0 failed, 6 ignored**;
`cargo clippy -p cbscore --all-targets`: clean).

---

## 1. Scope & what landed

C4a is the credential/object **infrastructure** layer, split out from the build
stages that consume it (plan review S-1). Delivered:

- **Vault client** (`utils/vault.rs`, new): `vaultrs`-backed; auth order AppRole
  → userpass → token; KV v2 mount `ces-kv` pinned; `read_secret`,
  `check_connection`; `from_config`. Built and connection-verified by
  `Builder::build`.
- **Vault-backed git resolution** (`utils/secrets/git.rs`): the `vault-ssh` /
  `vault-https` arms now read the `ces-kv` `key`, index the named credential
  fields, and reuse the existing plain materialisation.
- **S3 client** (`utils/s3.rs`, new): `aws-sdk-s3` with explicit static creds,
  scheme-normalised custom endpoint, placeholder region,
  `force_path_style(true)`; write path (`s3_upload_str_obj` / `_json` /
  `_files`) and the download primitive (`s3_download_str_obj`, 404 → `None`).
- **`s3_creds`** (`utils/secrets/storage.rs`, new) + `SecretsMgr::s3_creds`;
  shared `read_vault_secret` / `vault_field` hoisted into `secrets/mod.rs`.
- **`ca-certificates`** added to the builder toolchain (`builder/mod.rs`).
- **CI gate** flipped from "shipped graph excludes the heavy deps" to "build the
  real static-musl `cbsbuild` and assert it still links with neither aws-lc-rs
  nor openssl-sys, with the edges genuinely present"
  (`.github/workflows/ci-cbsd-rs.yaml`).
- Plan progress table: C4a flipped `Pending → Done`.

This is a **post-landing** review; the verdict addresses whether the slice is
acceptable as landed and safe to build C4b on top of.

## 2. Verified-correct (the things scrutinised that hold)

Each of the following was checked against code + Python + design and is
**correct**:

- **Vault auth order** AppRole → userpass → token (`from_config`, matching
  `vault.py:165-184`); covered by unit tests.
- **KV v2 mount `ces-kv`** pinned as a constant; every read passes it.
- **403 → permission-denied**: `forbidden()` matches
  `ClientError::APIError { code: 403, .. }`, mapping both reads and logins like
  Python's `hvac.exceptions.Forbidden`.
- **No `.address()` panic.** Verified against `vaultrs-0.7.4`: the
  `VaultClientSettingsBuilder::address` setter does
  `Url::parse(..).map_err(..).unwrap()` (client.rs:191-198) — it panics on a bad
  URL. `Vault::from_config` parses the address up front with the **same**
  `url::Url::parse` and additionally requires an `http(s)` scheme, so any
  address reaching `client()` is guaranteed to parse. The up-front check is
  strictly stronger than the setter's; the panic is unreachable. Tested by
  `a_malformed_address_is_rejected_not_panicked`.
- **Per-call login, no caching** — a fresh authenticated client per
  `read_secret` (Python's `FIXME` reproduced verbatim).
- **No `Debug`/`Display` secret leak.** `VaultAuth` and `Vault` derive no
  `Debug`; `aws_sdk_s3` `Credentials` redacts its secret key in `Debug`.
  `VaultError` carries only the (non-secret) address; secret values never enter
  a message.
- **Vault-git field-name indexing + `rstrip`.** `vault-ssh`/`vault-https` treat
  `ssh_key`/`username`/`password` as field **names** indexed into the secret,
  trailing-trimmed via the shared `vault_field` (`.trim_end()` == Python
  `.rstrip()`). The C4a-2 hoist of `read_vault_secret` / `vault_field` from
  `git.rs` into `secrets/mod.rs` is a pure move — git.rs behaviour is
  byte-identical (verified from the per-commit diff).
- **S3 client shape** (design 005 / invariant 9): explicit
  `Credentials::new(...)` static provider (never the default chain),
  scheme-normalised `endpoint_url`, `Region::new("us-east-1")` placeholder,
  `force_path_style(true)`, `behavior_version(latest)` (with the matching cargo
  feature, so no runtime panic).
- **`startswith("http")`** endpoint normalisation reproduced faithfully (incl.
  the quirk that a literal `http…` hostname is left unprefixed); tested.
- **`s3_download_str_obj` 404 → `None`** via both the typed `is_no_such_key()`
  service error **and** a raw HTTP-404 fallback — broader than Python and
  matching design 005's "map a 404 response to `None`". Content-type mismatch
  errors; body decoded UTF-8.
- **`s3_upload_files`** applies `public-read` only when `public`; builds the
  client once for the whole batch (Python opens one session for the loop). The
  ignored round-trip exercises both `public` branches and
  `ByteStream::from_path`.
- **S3 system trust store.** Verified against `aws-smithy-http-client-1.1.13`:
  `TrustStore::default()` sets `enable_native_roots: true` (tls.rs:131-137), and
  the rustls provider calls `rustls_native_certs::load_native_certs()` when
  enabled (rustls_provider.rs:114,157). The S3 client builds `build_https()`
  with the default context, so a system-installed CA **is** honoured — design
  012's native-roots intent is met for S3.
- **`s3_creds` exact-key lookup** (not longest-prefix), returning the lookup
  `url` as the hostname like Python; tested (`lookup_is_exact_not_prefix`).
- **CI `cargo tree -i` exit semantics.** Correct in both directions: the
  forbidden deps use `if cargo tree -i openssl-sys; then FAIL` (present → exit 0
  → fail), and the required edges use `if ! cargo tree -i aws-sdk-s3; then FAIL`
  (absent → non-zero → fail). `cbsbuild` depends on `cbscore`, which now pulls
  both heavy crates, so the "edges genuinely present" assertion holds.
- **Commit hygiene.** Each commit compiles alone: C4a-1's `Cargo.toml` adds only
  `vaultrs` + `url` (no aws-sdk-s3), and its `mgr.rs` does not reference the
  `storage` module; C4a-2 adds aws-sdk-s3 + aws-smithy-http-client and wires
  `s3_creds`. No new library `unwrap`/`expect`/`panic` outside tests
  (rust-2024). Clippy clean; 106 tests pass.

## 3. Findings

### M1 (Medium) — Vault TLS trust diverges from design and is unratified

The Vault client uses `vaultrs = { features = ["rustls"] }`, which maps to
`reqwest/rustls-tls` — **webpki bundled roots** plus `VAULT_CACERT` /
`VAULT_CAPATH` (verified in `vaultrs-0.7.4` client.rs:52-54,89,105, 244-248). It
does **not** read the system trust store.

Design 012's carve-out and the C4a plan both call for the **system trust store**
(`rustls-native-certs`) so that an operator's internal CA, installed system-wide
(the builder ships `ca-certificates`), is honoured. The S3 client satisfies this
(verified above); the Vault client does not. This produces a real **operational
asymmetry**: a private CA dropped into `/etc/pki` is trusted by the S3 path but
**not** the Vault path, which additionally requires `VAULT_CACERT` to be
exported. The commit message's "`ca-certificates` … resolve TLS against the
system trust store" is true for S3 but misleading for Vault.

The divergence is documented only in the commit message and the `vault.rs`
module doc — it is **not** ratified in the authoritative design (004 still says
the trust source "prefer[s] the system store"; 012 frames the system store as
the internal-CA mechanism). Per the project rule "if code and a design disagree,
fix the code or amend the design with a review," a commit-message note is
insufficient. Within `vaultrs` 0.7's feature set native roots are not reachable
without `native-tls` (which pulls `openssl`, violating the musl-clean gate), so
the divergence is partly a **third-party constraint** — which is exactly why it
should be recorded in the design and surfaced to operators, not buried in a
commit body. Public Vault endpoints still verify (webpki); only the private-CA
case is affected.

- Action: amend design 004 (and 012's carve-out) with a fidelity note that the
  Vault client uses webpki + `VAULT_CACERT`, and document the `VAULT_CACERT`
  requirement for private-CA deployments in the operator README. _(D8
  spec/design deviation; D11 operator-doc gap.)_

### M2 (Medium) — S3 critical paths have no CI coverage

`s3_upload_str_obj` / `_json` / `_files` and `s3_download_str_obj` (and the
`s3_creds` storage path for the vault-s3 arm) have **no production caller** in
this slice and are exercised only by the **`#[ignore]`d** live MinIO round-trip
(`s3_round_trip_put_get_and_missing_key`). The sole CI-run S3 test is
`normalize_endpoint`. So the upload/download/signing logic — the actual S3
behaviour — has **zero automated regression coverage** in default CI.

This is the deliberate, plan-approved "IO-primitive validated by live
round-trips" pattern (plan S-1/S-7), and the slice as a whole still delivers a
real capability because the **Vault half (C4a-1) is wired to a genuine
consumer** (`vault-git` via `prepare_components` → `Builder::build`). But C4a-2
in isolation delivers no operator-visible capability, and its core paths stay
unproven until C4b adds a consumer. Not compiler-dead code (the items are `pub`
and the ignored test references them; no `#[allow]` was needed), so this is a
**test-coverage** and commit-boundary observation, not a dead-code defect.

- Action: run the live MinIO and Vault round-trips (the acceptance gate the plan
  itself names) and record them green before C4b relies on the S3 client. _(D5
  untested critical path.)_

### M3 (Low/Medium) — Plan C4a scope narrowed but flipped to "Done"

The plan's C4a prose specifies "**SecretsMgr (complete)** … `s3_creds`,
`gpg_signing_key(id)`, `has_*` predicates" and "vault-backed resolution for
every family." Delivered: `s3_creds` and the vault-git completion only.
`gpg_signing_key`, `registry_creds`, `transit`, and every `has_*` predicate are
**absent** (deferred to C4b/C5/C6).

The deferral itself is **correct** and respects the no-dead-code rule — those
resolvers would have no caller until their consumer commits, so landing them in
C4a would be exactly the dead-code anti-pattern the project avoids. The issue is
**tracking**: the plan's C4a text still says "complete" and the progress row was
flipped to `Done`, overstating what landed. The plan should be reworded so the
C4a row reads "Vault + vault-git + `s3_creds` + S3 client" and the deferred
resolvers are listed against their consumer commits. _(D10
convention/tracking.)_

### L1 (Low) — Vault auth non-empty field validation dropped

Python's `VaultAppRoleBackend` / `UserPassBackend` / `TokenBackend` validate
non-empty `role_id`/`secret_id`/`username`/`password`/`token` at construction
and raise a clear `VaultError` ("missing role id", etc.). `Vault::from_config`
performs no such check — an entry present but empty is accepted and defers to a
login failure with a less specific error. A benign fidelity gap (empty creds
still fail, just later and less clearly). _(D8 fidelity deviation.)_

### L2 (Low) — New IO modules emit no structured logging

`vault.rs`, `s3.rs`, `storage.rs`, and the git vault arms return errors without
any `tracing` on their own paths; Python logs at each error and a `debug` on a
successful secret read. The error **cause** is preserved via `thiserror`
`#[source]` chains and logged once by the top-level consumer (builder/runner),
which is the port's consistent "errors propagate, consumer logs once" philosophy
— so this is a single architectural note, not a per-path defect. Worth
confirming the consumer actually logs the chained cause when a vault/S3 op fails
deep in a build. _(D9 observability.)_

## 4. Duplication, production risk, next-phase risk

- **Duplication:** none introduced. The opposite — C4a-1 extracted `ssh_git_url`
  / `https_git_url` helpers shared by the plain and vault arms, and C4a-2
  hoisted `read_vault_secret` / `vault_field` so git and storage share one
  implementation. Clean.
- **Production risk:** no secret-leak path found (no `Debug` on auth carriers;
  credentials wrapped/redacted; SDK errors carry only endpoint/key context). No
  public endpoint is added here. The only runtime-surprise is M1 (private-CA
  Vault needs `VAULT_CACERT`).
- **Next-phase risk (C4b):** C4b wires the production consumers (sign + upload,
  release RMW) onto C4a's S3 client and `s3_creds`. The risk is that the S3
  paths reach production having only ever run under the ignored test (M2) —
  mitigated by running the round-trips before C4b. `s3_download_str_obj` is
  correctly placed here for C4b's `release_desc_upload` RMW, so no rework is
  needed there.

---

## 5. Verdict

**GO (conditional).** The slice is a faithful, careful port: it compiles alone
per commit, lints clean, passes 106 tests, reproduces the Python behaviour (auth
order, `ces-kv`, 404→`None`, exact-key storage, field-name indexing + rstrip),
and the two load-bearing third-party claims (no `.address()` panic; S3 system
trust store) are verified true against the vendored crates. The C4a-1/C4a-2
split is sound: C4a-1 delivers a real end-to-end capability (vault-git clone)
and C4a-2 is the documented, sizing-driven IO-primitive commit.

No finding is a correctness or security blocker for the commits as landed.
Before/within C4b, address: (1) **M1** — amend designs 004/012 to ratify the
Vault webpki + `VAULT_CACERT` trust source and document the operator
`VAULT_CACERT` requirement; (2) **M2** — run and record the live MinIO/Vault
round-trips that are this slice's named acceptance gate; (3) **M3** — reword the
plan's C4a scope so "Done" reflects what actually landed.

## 6. Confidence score

| Item                                                                                                          | Points |
| ------------------------------------------------------------------------------------------------------------- | ------ |
| Starting score                                                                                                | 100    |
| M1 / D8: Vault TLS trust (webpki+VAULT_CACERT) diverges from 004/012 system-store requirement                 | -5     |
| M1 / D11: private-CA Vault `VAULT_CACERT` requirement undocumented for operators                              | -5     |
| M2 / D5: S3 upload/download paths covered only by an `#[ignore]`d live test (no CI coverage)                  | -15    |
| M3 / D10: plan C4a "SecretsMgr complete" narrowed (gpg/registry/transit/`has_*` deferred) yet flipped to Done | -5     |
| L1 / D8: Vault auth non-empty field validation dropped vs Python                                              | -5     |
| L2 / D9: new IO modules emit no structured logging on their own paths                                         | -5     |
| **Total**                                                                                                     | **60** |

Interpretation: 60 sits in the "significant issues — address before proceeding"
band, but every deduction is a **follow-up or tracking** item, not a defect in
the landed code. The score is dominated by M2's untested critical path (the
price of the infra-ahead-of-consumer split) and M1's unratified design
divergence. Functionally the slice is clean; the verdict is GO with the three
actions above tracked into C4b.

## 7. Findings ordered by severity

1. **M1 (Medium)** — Vault TLS trust uses webpki + `VAULT_CACERT`, not the
   system trust store; diverges from designs 004/012, unratified, and
   undocumented for operators (private-CA asymmetry vs S3).
2. **M2 (Medium)** — S3 client has no production consumer and its core
   upload/download paths run only under an `#[ignore]`d live test; no CI
   regression coverage until C4b.
3. **M3 (Low/Medium)** — plan C4a scope ("SecretsMgr complete",
   `gpg_signing_key`, `has_*`) narrowed by a correct no-dead-code deferral, but
   the row was flipped to Done without rewording.
4. **L1 (Low)** — Vault auth backends no longer validate non-empty credential
   fields at construction (Python did).
5. **L2 (Low)** — new IO modules (`vault`/`s3`/`storage`) emit no structured
   logging on their own error paths.
