# Review — crt: select the signing key by name and read custom Vault fields

- **Type:** implementation review (v1)
- **Target:** commit `867db44` on `wip/release-tool-v2`
- **Design:** `cbsd-rs/docs/crt/design/001-20260620T1318-v2-mvp.md` (§6.1, §9)
- **Reviewer mandate:** distrust implementer claims; verify against source,
  including the `vaultrs-0.8.0` crate source, and run the test/lint suite.
- **Date:** 2026-06-27

## Scope

A single commit that makes the OpenPGP signing key selectable and its Vault
field names configurable:

- `crt/src/secrets.rs` — `vault.keys` changes from `BTreeMap<String, String>` to
  `BTreeMap<String, VaultKeyEntry>`; new
  `VaultKeyEntry { path, private_key_field, passphrase_field }` with field-name
  defaults `private-key` / `passphrase`.
- `crt/src/config.rs` — new `gpg_private_key: Option<String>` plus a
  `gpg_private_key_name()` resolver defaulting to
  `DEFAULT_GPG_KEY_NAME = "gpg_signing_private"`.
- `crt/src/vault.rs` — `fetch_signing_key(&VaultSecrets, key_name)` looks up the
  named entry, reads the KV data into `serde_json::Map<String, Value>`, and a
  pure `extract_signing_key(data, entry)` pulls the configured fields. The
  hardcoded `SigningKeySecret` struct and `SIGNING_KEY_NAME` constant are gone.
- `crt/src/main.rs` — both call sites (seal + materialize) pass
  `cfg.gpg_private_key_name()`.
- `crt/src/{release.rs,verify.rs}` — test `Config` constructors gain
  `gpg_private_key: None`.
- Docs: `README.md`, `crt.config.yaml.example`, `crt.secrets.yaml.example`.

## Verdict

**GO.** The change is correct, well-tested, idiomatic, and the docs match the
code exactly. It is an intentional breaking change to a pre-release tool's
config schema, which is acceptable at this stage. The findings below are all
low-impact polish items; none block the commit.

## What was verified (not assumed)

### 1. `vaultrs::kv2::read` returns the inner KV data dict — CONFIRMED

This was the load-bearing correctness question. Verified against the crate
source rather than assumed:

- `vaultrs-0.8.0/src/kv2.rs:107-119` — `read::<D>()` calls
  `api::exec_with_result(...)` then `serde_json::value::from_value(res.data)`.
- `vaultrs-0.8.0/src/api/kv2/responses.rs:17-21` — the endpoint response type is
  `ReadSecretResponse { data: Value, metadata: SecretVersionMetadata }`.
- `vaultrs-0.8.0/src/api/kv2/requests.rs:56-71` — `ReadSecretRequest` targets
  `{mount}/data/{path}` with `response = "ReadSecretResponse"`.
- `vaultrs-0.8.0/src/api.rs:286-303` + `strip` (366-377) — `exec_with_result`
  strips the **outer** Vault envelope
  (`EndpointResult { data, wrap_info, warnings, auth }`) and returns the typed
  `ReadSecretResponse`.

So the two Vault wrapping layers resolve cleanly: the outer envelope is stripped
by `strip`, and `ReadSecretResponse.data` is the inner KV-v2 `data.data` object
— i.e. exactly the user's field dict. Deserializing that `Value` into
`Map<String, Value>` yields the raw fields. The implementer's approach is sound.

### 2. `extract_signing_key` field handling — CORRECT

`vault.rs:32-50`:

- Missing key field **or** non-string key value → `Err` (`.as_str()` yields
  `None`, and `anyhow::Context::with_context` on `Option` converts `None` to an
  error with `field "<name>" is missing or not a string`). Non-string is an
  error, not silently wrong. Confirmed `anyhow::Context` is implemented for
  `Option`.
- Absent passphrase field → `None` (correct; passphrase is optional).
- The three unit tests (`vault.rs:242-265`) are non-tautological: they exercise
  custom field names, absent-passphrase → `None`, and a key field whose name
  does not match the stored field → error.

### 3. Selector + lookup — CORRECT

- `config.rs:142-145` — `gpg_private_key_name()` returns the configured name or
  `DEFAULT_GPG_KEY_NAME` (`gpg_signing_private`, `config.rs:48`), matching the
  prior hardcoded constant so an unconfigured deployment keeps the old default
  entry name.
- `vault.rs:57-60` — a missing named entry errors clearly:
  `secrets \`vault.keys\` has no "<name>" entry`.
- Both call sites (`main.rs:398`, `main.rs:446`) pass
  `cfg.gpg_private_key_name()`; no stale `SIGNING_KEY_NAME` / `SigningKeySecret`
  references remain anywhere in `crt/src`.

### 4. serde defaults + old-form behavior — VERIFIED EMPIRICALLY

- `VaultKeyEntry` field-name defaults resolve to `private-key` / `passphrase`
  when omitted (`secrets.rs:61-73`; test `a_key_entry_defaults_the_field_names`,
  `secrets.rs:296-305`).
- The **old** config shape `keys: { gpg_signing_private: <path-string> }` now
  fails to parse. Verified with a throwaway probe test (since reverted): the
  error is `type mismatch: expected mapping, found other`, wrapped by
  `secrets::load` as `parsing secrets <path>`. It fails loudly (no silent
  mis-parse) — see finding F1 on message clarity.

### 5. No auth regressions — CONFIRMED

The auth path (token / userpass / approle) is byte-for-byte unchanged; only the
`fetch_signing_key` signature changed. All three `#[ignore]` live tests and the
shared `live_vault_base()` helper were updated to the new `VaultKeyEntry` shape
and pass the `TEST_KEY_NAME` constant (`vault.rs:151-230`). `cargo test -p crt`
compiles all targets including the ignored tests.

### 6. Security — CONFIRMED no new leak

- `SigningKey` (`vault.rs:23-27`) has **no** `#[derive(Debug)]` — the armored
  key and passphrase cannot be accidentally formatted.
- `grep` for `{:?}` / `println!` / `eprintln!` / `dbg!` / `tracing` in
  `vault.rs` and `main.rs`: the only `{:?}` sites format `entry.path` and
  `entry.private_key_field` (a path and a field **name** — neither secret). No
  print/log statement touches `SigningKey`, the fetched `data` map, or a
  passphrase.
- `VaultSecrets` does derive `Debug` and holds `token`, but nothing in the
  changed code prints it. This is **pre-existing** and not introduced or
  worsened by this commit.

### 7. Docs accuracy — CONFIRMED, no drift

- `README.md:163-214` and both `.example` files use snake_case field names
  (`private_key_field`, `passphrase_field`) matching the struct fields, state
  the correct defaults (`private-key` / `passphrase`), and describe the
  `gpg_private_key` default as `gpg_signing_private` (matches
  `DEFAULT_GPG_KEY_NAME`).
- The mount/`data`-infix description matches `split_kv2_path`
  (`vault.rs:109-123`): `ces-kv/gpg/pvt` → mount `ces-kv`, path `gpg/pvt`,
  endpoint `ces-kv/data/gpg/pvt`.
- `crt.config.yaml.example:26` (`gpg_private_key: release-signing`) and
  `crt.secrets.yaml.example:33` (`keys: { release-signing: ... }`) are mutually
  consistent — copying both examples yields a working configuration.

### Tooling

- `cargo test -p crt`: **75 passed, 0 failed, 5 ignored**.
- `cargo clippy -p crt`: clean (no warnings).
- `cargo fmt -p crt --check`: clean.

## Confidence Score

| Item                                             | Points | Description                                                                                                                                 |
| ------------------------------------------------ | ------ | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                   | 100    |                                                                                                                                             |
| F1 (D9) — opaque migration error                 | -5     | Old-form config fails with a generic `type mismatch: expected mapping` that names neither the offending `keys` entry nor the migration path |
| F2 (D5) — non-string key value path untested     | -0*    | Same code branch as the tested missing-field case; thoroughness nit, not an uncovered path                                                  |
| F3 (D4) — non-string passphrase silently dropped | -5     | A present-but-non-string passphrase field becomes `None` rather than erroring, asymmetric with the key field's strict handling              |
| **Total**                                        | **90** |                                                                                                                                             |

\* F2 does not deduct: the non-string-key branch is the identical `.as_str()` →
`None` → error path already covered by
`extract_errors_when_the_key_field_is_missing`, so it is a
documentation-of-intent gap rather than a genuinely untested critical path.
Recorded for completeness.

**Interpretation: 90/100 — ready to merge.** Minor polish only.

## Findings (by severity)

### F1 — Opaque error when upgrading from the old `keys` schema (low)

`secrets.rs:50,58-65`. The schema change from `BTreeMap<String, String>` to
`BTreeMap<String, VaultKeyEntry>` is an intentional, acceptable breaking change.
But an operator carrying the old
`keys: { gpg_signing_private: secret/data/... }` shape gets only
`parsing secrets <path>: type mismatch: expected mapping, found other` (verified
empirically). The message names neither the offending key nor how to migrate.
Optional improvement: a note in the upgrade/README path, or a custom deserialize
that detects a bare string and suggests the `{ path: ... }` form. Not blocking —
it fails loudly and safely.

### F2 — Non-string key value relies on the missing-field test (informational)

`vault.rs:33-41`, tests at `vault.rs:242-265`. A key field present but holding a
non-string JSON value (number/object/array) correctly errors via the same
`.as_str()`-returns-`None` branch as a missing field. There is no dedicated test
asserting the non-string case, though the error message explicitly mentions "or
not a string". Behaviorally covered; a one-line test would document the intent.
No deduction.

### F3 — Non-string passphrase silently treated as absent (low)

`vault.rs:42-45`. The passphrase read uses `.and_then(|v| v.as_str())` then
`.map(...)`, so a passphrase field present with a non-string value yields `None`
(treated as "no passphrase") rather than an error — asymmetric with the strict
handling of the key field. Low impact: a non-string passphrase is an unusual
misconfiguration, and a key with a passphrase but read as passphrase-less will
fail later at the signing step. Worth a comment or a symmetric error if strict
parity is desired.

## Commit hygiene (git-commits / D12)

PASS. One logical change (key selection + custom field names are a single
coherent capability). The message leads with the "why" — a deployment whose KV
secret stores the key under a non-default field could not be used at all — then
states the "what". The 204-line diff is inflated by doc edits (README + two
examples); authored Rust is modest and tightly scoped. The commit compiles,
tests pass, and it is cleanly revertable. Trailers (`Co-authored-by`,
`Signed-off-by`) are present as expected for this repo.

## Recommended actions (all optional, non-blocking)

1. (F1) Add an upgrade note for the `keys` schema change, or detect the old bare
   string form and emit a migration hint.
2. (F3) Decide whether a non-string passphrase should error; if so, mirror the
   key field's `with_context` handling. Otherwise add a brief comment that a
   non-string passphrase is intentionally treated as absent.
3. (F2) Optionally add a unit test for a non-string key value to document
   intent.
