# Implementation review — crt Vault userpass + AppRole auth (v1)

- **Commit reviewed:** `950edae` — _crt: authenticate to Vault with userpass or
  AppRole, not just a token_
- **Scope:** the single commit's diff: `crt/src/secrets.rs`, `crt/src/vault.rs`,
  `crt/crt.secrets.yaml.example`, `crt/README.md`
- **Reviewer mandate:** distrust the implementer; verify every API claim against
  the real `vaultrs-0.8.0` source; hunt for credential leaks, wrong API usage,
  doc drift, parity divergence.
- **Verdict:** **GO** — confidence **84 / 100**.

---

## Summary

The change lifts crt's Vault authentication from token-only to a three-method
resolver (token / userpass / AppRole), mirroring cbscore. The core design is
clean: a `VaultAuth<'a>` borrow enum, a single `VaultSecrets::auth()` validation
point enforcing _exactly one_ method, and `fetch_signing_key` that seeds the
client token only for the token variant and otherwise logs in and `set_token`s
the returned client token before the KV v2 read.

Every load-bearing external-API claim was verified against the crate source and
holds. The code compiles, clippy is clean, and all 67 offline tests pass (5 live
tests correctly `#[ignore]`d). No credential leak path was introduced. The
findings below are all low-severity polish; none blocks merge.

---

## Verification performed (claims checked against primary sources)

| Claim in the code                                              | Verified against                                                                                                        | Result                                        |
| -------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| `userpass::login(client, mount, username, password)` arg order | `vaultrs-0.8.0/src/auth/userpass.rs:9-22`                                                                               | **Matches** call site `vault.rs:71`           |
| `approle::login(client, mount, role_id, secret_id)` arg order  | `vaultrs-0.8.0/src/auth/approle.rs:12-25`                                                                               | **Matches** call site `vault.rs:78`           |
| `AuthInfo` exposes `client_token: String`                      | `vaultrs-0.8.0/src/api.rs:63-64`                                                                                        | **Confirmed**                                 |
| `Client::set_token(&mut self, &str)` exists                    | `vaultrs-0.8.0/src/client.rs:28,76`                                                                                     | **Confirmed**                                 |
| `.build()` succeeds without `.token(...)`                      | `client.rs:188` (`default = self.default_token()`), `validate()` at `client.rs:344` only checks the URL                 | **Confirmed** — empty token passes `.build()` |
| A failed `login` does **not** echo credentials                 | `vaultrs-0.8.0/src/error.rs:5-7` — `ClientError::APIError { code, errors }` carries only Vault's status + error strings | **Confirmed** — no leak                       |
| `fetch_signing_key` callers propagate errors                   | `crt/src/main.rs:397` (seal), `:444` (materialize) — both `?` into anyhow                                               | **Confirmed**                                 |
| Resolver tests are non-tautological                            | `secrets.rs:177-228` — assert on variant + field values + default mount                                                 | **Confirmed**                                 |
| Compile / clippy / tests                                       | `cargo test -p crt --bins` (67 pass, 5 ignored); `cargo clippy -p crt --bins --tests` (clean)                           | **Pass**                                      |

### Resolver correctness (the exactly-one rule)

`VaultSecrets::auth()` (`secrets.rs:95-121`) counts the three `Option` fields
and matches on `(count, resolved)`:

- 0 set → `(0, _)` → clear "no auth method" error. **Correct.**
- exactly 1 → `(1, Some)` → the resolved variant. **Correct.**
- 2 or 3 set → falls to the `_` arm → "sets {count} auth methods". **Correct** —
  `count` is interpolated, so the message is accurate (reports 2 or 3).

Walked all four cases; the logic is sound and the error text is honest. The five
unit tests exercise token, userpass-with-default-mount,
approle-with-explicit-mount, zero (error), and two (error) — each asserting real
values, not just `is_ok()`.

### Security

- No `VaultSecrets` / `VaultUserPass` / `VaultAppRole` instance is ever
  `Debug`-printed. A repo-wide sweep of `{:?}` / `eprintln!` / `println!` in
  `crt/src/` found only unrelated uses (git args, config component names, risk
  bands). The `Debug` derives on the credential structs are inert.
- The **new** error path (login failure → `.context(...)` → anyhow `{:#}` in
  `main`) cannot leak the password/secret_id: `ClientError::APIError` carries
  only `code` + Vault's `errors` array (`error.rs:6-7`), and Vault does not echo
  submitted credentials.
- The plaintext-credential-in-YAML trade-off was previously accepted; the
  `chmod 600` warning in `warn_if_too_open` (`secrets.rs:133-145`) is unchanged.
  No new on-disk or on-wire exposure.

### Docs cross-check (field-by-field, no drift)

`README.md` table + both YAML blocks and `crt.secrets.yaml.example` were checked
against the structs:

- `username` / `password` (`VaultUserPass`), `role_id` / `secret_id`
  (`VaultAppRole`), `mount` on both — **all snake_case, all match.**
- Default mounts documented as `userpass` / `approle` — match
  `default_userpass_mount` / `default_approle_mount` (`secrets.rs:73-79`).
- "set **exactly one** … (more than one, or none, is rejected)" — matches
  `auth()` semantics. **No drift found.**

### Parity with cbscore (`cbscore/utils/vault.py`)

The "mirrors cbscore's three methods" claim is accurate for the _methods_. Two
**intentional supersets** (not findings) are worth recording so the parity claim
is qualified correctly:

1. crt exposes a configurable `mount` per method; cbscore hardcodes hvac's
   default mounts. Superset — fine.
2. crt **rejects** more than one method; cbscore is first-wins (`auth_approle` →
   `auth_user` → `auth_token`) and only errors on _none_. crt is stricter — a
   deliberate improvement.

One genuine **divergence** (finding F3 below): cbscore validates each credential
is **non-empty** (`if not token: raise "missing token"`, etc.); crt's `auth()`
validates only the _presence of the section_.

---

## Findings (by severity)

No Critical or Important findings. All Minor.

### F1 — `fetch_signing_key` orchestration has no offline test (D5, conf 70)

`vault.rs:45-93`. The new control flow — build token-less → `login` →
`set_token` → `kv2::read` — is exercised **only** by the three `#[ignore]` live
tests (`vault.rs:166-210`), which never run in CI. The resolver (`secrets.rs`)
is well covered, but the orchestration in `fetch_signing_key` is the new
critical path and has zero offline coverage.

**Mitigation (strong):** `vaultrs` is not trivially mockable (the `login` free
functions take `&impl Client` and hit the network), and the _pre-existing_ token
fetch was likewise live-only. This is **not a regression** — it preserves the
prior testing posture. The `#[ignore]` gating and per-method env-var docs are
correct. Recommend (non-blocking) a follow-up wiremock-style HTTP fixture if
offline coverage of the login leg is later desired.

### F2 — doc-comment overstates the token default as "empty" (D11, conf 80)

`vault.rs:54-56`: _"the builder defaults the token to empty, so `.build()`
succeeds without it."_ Per `client.rs:258-268`, `default_token()` falls back to
the **`VAULT_TOKEN` environment variable** and only then to empty string. The
comment's _conclusion_ (`.build()` succeeds without an explicit token) is
correct, but the stated reason is imprecise. Functionally harmless — `set_token`
overwrites whatever the builder picked up before the KV read, so even a stray
`VAULT_TOKEN` in the environment cannot affect the userpass/AppRole paths.
Suggest: "the builder defaults the token to `$VAULT_TOKEN` or empty; either way
`set_token` below overrides it."

### F3 — empty-string credentials pass `auth()` but fail late (D8/parity, conf 50)

`secrets.rs:95-121`. `auth()` checks only that a section is _present_, not that
its fields are non-empty. cbscore rejects empties upfront with a precise message
(`missing token` / `missing role id` / …). In crt:

- `token: ""` — present-but-empty; counts as one method, passes `auth()`, then
  fails at the Vault call with a vaguer error.
- `userpass:` / `approle:` with empty `username`/`password`/`role_id`/
  `secret_id` — serde _requires_ those fields (they are not `Option`), so they
  cannot be _missing_, but they can be empty strings.

Low impact: the only "present-but-empty" surprise is the `token` case, and Vault
still rejects it (just with a less friendly message than cbscore's upfront
check). Optional hardening: have `auth()` reject blank credentials to match
cbscore's messages.

---

## Confidence score

| Item                                                              | Points | Description                                                                                        |
| ----------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------- |
| Starting score                                                    | 100    |                                                                                                    |
| F1 — D5: `fetch_signing_key` login orchestration untested offline | -15    | New critical path; only `#[ignore]` live tests. Mitigated (not a regression; vaultrs not mockable) |
| F2 — D11: token-default doc-comment imprecise                     | -5     | "defaults to empty" is really "`$VAULT_TOKEN` or empty"; conclusion still correct                  |
| F3 — D8: empty-string credential not rejected (cbscore parity)    | -5     | `auth()` checks section presence, not non-empty fields; `token: ""` slips through                  |
| **Total**                                                         | **84** |                                                                                                    |

Interpretation: **75–89 — acceptable with noted improvements.** None of the
deductions is a blocker.

---

## Commit quality (git-commits)

- **One logical change:** yes — a single capability (accept userpass/AppRole in
  addition to token), spanning the schema, the resolver, the fetch shim, and its
  docs. Coherent and revertable.
- **Message leads with _why_:** yes — opens with the operator pain (operators
  who log in via userpass/AppRole were forced to mint a token by hand) before
  the _what_. Subject
  `crt: authenticate to Vault with userpass or AppRole, not just a token` is <
  72 chars, descriptive, component-prefixed.
- **Sizing:** 298 changed lines, but ~61 are docs (README + example) and a large
  share of the rest is tests; authored non-test/non-doc code is well under the
  400-line floor. Appropriate for the scope — no split warranted.
- **Compiles & tests pass alone:** verified (`cargo test -p crt --bins`, clippy
  clean). No dead code: `VaultAuth` and the new structs all have callers within
  the commit. **Passes the smell test.**

One nit (not a code finding): the trailer reads
`Co-authored-by: Claude Opus 4.8 (1M context)`, whereas `cbsd-rs/CLAUDE.md`
shows the canonical form as the bare model name
(`Claude Sonnet 4.6 <noreply@anthropic.com>`). Cosmetic; flagging for
consistency only.

---

## Recommended actions before next stage (all optional)

1. (F2) Reword the `vault.rs:54-56` comment to say `$VAULT_TOKEN`-or-empty.
2. (F3) Optionally reject blank credentials in `auth()` to match cbscore's
   precise "missing X" messages.
3. (F1) If offline coverage of the login leg is wanted later, add an
   HTTP-fixture test; not required now.
