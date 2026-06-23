# Review — 004 Configuration, secrets & Vault (v1)

Adversarial design review of
`cbsd-rs/docs/cbscore-rs/design/004-20260621T1234-configuration-secrets-and-vault.md`.

Reviewed as a standalone design artifact against the Python source of truth.
Every claim was checked against the real code; nothing was taken on the author's
word. The settled decisions (cbscore-owned formats carry a `schema-version`
marker; no `config init`; IO-layer errors live in this subsystem; Vault auth
order AppRole → userpass → token on mount `ces-kv`) were treated as fixed and
only checked for honoring, not relitigated.

Sources consulted:

- `cbscore/src/cbscore/config.py`
- `cbscore/src/cbscore/utils/secrets/models.py`
- `cbscore/src/cbscore/utils/secrets/mgr.py`
- `cbscore/src/cbscore/utils/secrets/git.py`
- `cbscore/src/cbscore/utils/secrets/storage.py`
- `cbscore/src/cbscore/utils/secrets/signing.py`
- `cbscore/src/cbscore/utils/secrets/registry.py`
- `cbscore/src/cbscore/utils/secrets/utils.py`
- `cbscore/src/cbscore/utils/vault.py`
- `cbscore/src/cbscore/utils/__init__.py`

## Verdict

**Go with changes.**

The design is largely faithful: the config types, kebab keys, optionality, Vault
auth order and mount, the `ces-kv` constant, `read_secret`'s `data.data` shape,
the secrets container's four maps (with the correct `sign` key, not `signing`),
and four of the five resolver return tuples all match the Python exactly. Every
defect below is a correctable spec fix, not a structural dead end — hence
go-with-changes rather than no-go. Three findings are must-fix before
implementation begins (F1, F2, F3); one is must-justify (F4).

## Confidence score

| Item                                                       | Points | Description                                                                          |
| ---------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------ |
| Starting score                                             | 100    |                                                                                      |
| F1 — `transit()` return tuple inverted vs Python           | -5     | D8 spec deviation: design says `(key, mount)`; Python returns `(mount, key)`         |
| F2 — secret resolution lookup strategy unspecified/wrong   | -20    | D1 deferred: the `find_best_secret_candidate`/`matches_uri` matcher is absent        |
| F3 — credential families modeled as nested two-level enums | -5     | D8 spec deviation: Python is uniformly single-level combined-discriminator           |
| F4 — git `type:` wire change framed as necessary           | -5     | D8 spec deviation: only forced under the rejected framing; wrong alternative refuted |
| F5 — git ssh `git_url_for` guard scope mis-described       | -5     | D11 doc gap: design describes only the https/token path                              |
| F6 — signing field-shapes compressed in the table          | -5     | D11 doc gap: four gpg variants have distinct shapes, shown as one                    |
| **Total**                                                  | **55** |                                                                                      |

55 lands in the "significant issues — must address before proceeding" band. The
dominant deduction is F2: a port built literally from the current text would
resolve git and registry secrets incorrectly for the common (non-exact-key)
case. F1 is small in points but produces a real mount/key swap bug. F3 and F4
are correctness-of-modeling issues that, if implemented as written, push the
port onto a serde representation that is both unfaithful to Python and on a
known sharp edge.

## Findings, ordered by severity

### F2 (critical) — the secret-resolution lookup strategy is missing and the stated one is wrong for git/registry

**Design claim.** "Resolution surface (each looks up by the map key, resolves
plain-or-vault, …)" (lines 179-180), then lists `s3_creds`, `registry_creds`,
`transit`, `git_url_for`, `gpg_signing_key`. The design nowhere mentions
URI-prefix matching.

**What the Python actually shows.** The three families do **not** share a lookup
strategy:

- storage uses an exact map lookup: `entry = secrets.get(host)`
  (`storage.py:33`). A missing exact key is an error.
- git uses fuzzy longest-prefix matching:
  `best_entry = find_best_secret_candidate(list(secrets.keys()), url)`
  (`git.py:244`); on no match it yields the URL unchanged (`git.py:252`), it
  does not error.
- registry also uses `find_best_secret_candidate` (`registry.py:60`).
- `find_best_secret_candidate` (`utils.py:20-48`) walks every key, calls
  `matches_uri(target, uri)`, returns a full match immediately, otherwise keeps
  the candidate with the shortest remainder (the longest matching prefix).

**The gap.** The whole `find_best_secret_candidate` / `matches_uri` matcher is
in scope (004 claims to be the reference for the per-family resolvers) and is
entirely absent. A port built from the design would do exact-key lookups for git
and registry and break resolution for any URL that is not byte-identical to a
stored key — i.e. the normal case, where the key is a host or host/prefix and
the lookup URL is a full repo URL. The design also flattens the three different
not-found behaviors (storage errors; git yields the bare URL; registry errors)
into one sentence.

**Recommended change.** Specify the lookup contract per family: storage = exact
key; git and registry = longest-prefix match via the ported
`matches_uri`/`find_best_secret_candidate`; git's no-match path returns the URL
unchanged (a bare `SecureUrl`) rather than erroring. Either fold `matches_uri`
into this design or cross-reference the design that owns it, but do not leave
the matcher unspecified.

### F1 (important) — `transit()` return tuple is inverted

**Design claim.** "`transit(id) -> (key, mount)` — Vault transit signing
config." (line 186).

**What the Python actually shows.** `SecretsMgr.transit` returns
`signing_transit(id, self.secrets.sign)` unchanged (`mgr.py:89-91`), and
`signing_transit` returns `(secret.mount, secret.key)` (`signing.py:184`). The
order is **`(mount, key)`**, not `(key, mount)`.

**The gap.** As written the design inverts the two fields. A caller wiring the
transit signing config from this tuple would pass the mount where the key is
expected and vice versa — a real, silent misbehavior. Note this is the lone
error among the resolver tuples: `s3_creds` → `(host, access_id, secret_id)`
(`storage.py:40,56`), `registry_creds` → `(address, username, password)`
(`registry.py:47,70`), and `gpg_signing_key` →
`(keyring_path, passphrase?, email)` (`signing.py:138`) all match the design.
That isolation is what makes F1 a clean, high-confidence fix.

**Recommended change.** Change line 186 to `transit(id) -> (mount, key)`.

### F3 (important) — the two-level tagged-enum modeling is unfaithful and lands on a serde sharp edge

**Design claim.** "The Rust port models each family as a **two-level tagged
enum** — outer tag `creds`, inner tag `type` — except registry (one level,
`creds` only)" (lines 136-137).

**What the Python actually shows.** Every family is a **single-level**
discriminated union driven by one custom discriminator that inspects both
`creds` and `type` (where present) and returns a single combined tag:

- storage: `pydantic.Discriminator(storage_secret_discriminator)` → `plain-s3` /
  `vault-s3` (`models.py:165-193`).
- signing: `pydantic.Discriminator(signing_secret_discriminator)` → one of five
  combined tags (`models.py:269-312`).
- git: `pydantic.Discriminator(git_secret_discriminator)` → one of five combined
  tags (`models.py:89-136`).
- registry: `pydantic.Discriminator(registry_secret_discriminator)` →
  `plain-registry` / `vault-registry` (`models.py:331-356`).

There is no nesting in Python: each family is flat, with a function that peeks
the relevant fields once and yields a single tag.

**The gap.** Two distinct problems.

(a) The "two-level / nested tagged enum" framing is not faithful to Python's
uniform single-level combined-discriminator structure. The design is also
internally inconsistent about it: registry is modeled single-level (`creds`
only) — which is exactly what Python does and exactly right — while storage,
signing and git are modeled two-level, even though Python treats all four
identically.

(b) Independently of fidelity, modeling a family as an internally-tagged enum
whose variants are themselves internally-tagged enums reading a second tag from
the same flat map is a documented serde sharp edge: internally-tagged enums
buffer the input into an intermediate content map and have known limitations
once nesting, `flatten`, or `deny_unknown_fields` enter the picture, and the
error messages such a construction produces are poor. The design should not bet
the implementation on this representation compiling and round-tripping cleanly.

**Recommended change.** Model each family single-level, mirroring Python: a
custom `Deserialize` (or a flat struct that reads `creds` + `type` + fields and
converts post-deserialize) that peeks both fields and selects the variant — i.e.
the direct Rust translation of the Python discriminator function. This is
faithful, robust regardless of the serde nesting edge, and uniform across all
four families (registry simply has no `type` to read).

### F4 (must-justify) — the git `type:` wire change is framed as necessary, but the design refuted the wrong alternative

**Design claim.** The git `type:` field is "the one operator-visible format
change in 004" and is presented as forced because "serde's tagged enums do not
inspect shape, and an `untagged` enum that guessed by shape is fragile and gives
unusable errors" (lines 155-169).

**What the Python actually shows.** The shape-discrimination claim itself is
accurate: `git_secret_discriminator` (`models.py:89-126`) inspects the entry for
`ssh-key` / `token` / `username`+`password` within the `creds` tag rather than
reading an explicit `type`. The five `(creds, type)` combinations the design
enumerates are exactly Python's five variants, and `token` is indeed plain-only
— there is no `vault-token` variant (`models.py:55-60` defines
`GitTokenSecret(PlainSecret)` only; the vault git variants are ssh and https).
So the factual basis is sound.

**The gap.** The conclusion that a wire change is _necessary_ only holds under
the serde-tagged-enum framing rejected in F3. The design refutes `untagged`
(line 162) — correctly, `untagged` is fragile — but `untagged` is not the
faithful mechanism. The faithful mechanism is the custom discriminator the port
needs for storage and signing anyway (F3); once that exists, git's shape-based
discrimination is just one more discriminator function
(`git_secret_discriminator` translated to Rust), and **no wire change is
needed**. The design rejected the wrong alternative and concluded a format break
was unavoidable.

**Recommended change.** Keep the explicit git `type:` only if it is justified on
its own merits (explicit tags can genuinely be clearer and give better errors
than shape-guessing). But (a) drop the "necessary" framing, and (b) weigh it
explicitly against the no-wire-change custom- discriminator path, since the
design's HARD question is precisely whether the tag is the right call versus a
faithful custom `Deserialize`. If the explicit tag is kept, the
operator-migration note and README requirement stand; if not, git needs no
operator change either, and the "one operator- visible format change" claim is
dropped.

### F5 (minor) — the git ssh `git_url_for` RAII-guard scope is mis-described

**Design claim.** "`git_url_for(url) -> Guard<CmdArg>` — a git URL with
credentials folded in (a `SecureUrl`, 003), or the URL unchanged when none."
(lines 187-188), and the Drop "removes the temporary material" (lines 194-200).

**What the Python actually shows.** The "URL with credentials folded in"
description fits only the https and token paths (`git.py:203-209`,
`git.py:224-230`), which build a `SecureURL`. The ssh path (`_ssh_git_url_for`,
`git.py:69-161`) does something different: it writes into `~/.ssh/known_hosts`
and `~/.ssh/config`, drops a temp key file, and yields an ssh-config **alias**
of the form `remote_name:repo` (a plain string, not a `SecureURL`). Its cleanup
unlinks only the temp key (`git.py:161`); the appended `~/.ssh/config` and
`known_hosts` entries are left behind.

**The gap.** The single "SecureUrl with creds folded in / Drop removes the
temporary material" description does not capture the ssh path, which is the one
case that actually materializes files and whose cleanup is narrower than
"removes the temporary material" implies. This matters for the RAII-guard
design: a faithful guard must decide what its `Drop` covers, and the Python
precedent leaves config/known_hosts entries in place.

**Recommended change.** Note that the ssh variant returns a config-alias string
and writes `~/.ssh/{config,known_hosts}`; state explicitly what the Rust guard's
`Drop` cleans (the temp key, matching Python) versus what it deliberately
leaves, so the implementer does not assume full teardown.

### F6 (minor) — signing field-shapes are over-compressed

**Design claim.** "signing: gpg variants
`{private-key, public-key?, passphrase?, email}` (public-only:
`{public-key, email}`); transit `{mount}`." (lines 151-152).

**What the Python actually shows.** The five signing variants do not share one
gpg shape: `GPGPlainSecret` and `GPGVaultSingleSecret` carry
`{private-key, public-key?, passphrase?, email}` (`models.py:196-228`);
`GPGVaultPrivateKeySecret` (`gpg-pvt-key`) has **no** `public-key`
(`models.py:230-243`); `GPGVaultPublicKeySecret` (`gpg-pub-key`) has only
`{public-key, email}` — no `private-key`, no `passphrase` (`models.py:246-258`);
`VaultTransitSecret` has `{mount}` (`models.py:261-266`).

**The gap.** The table correctly lists all five tags, so this is a completeness
nit, not a wrong tag set; but the field-shape line collapses four distinct gpg
shapes into one and only calls out the public-only case.

**Recommended change.** Give the per-variant field shapes (or at least note that
`gpg-pvt-key` drops `public-key` and `gpg-pub-key` carries only `public-key` +
`email`), so the implementer does not model a uniform gpg struct.

## Confirmed accurate — do not change

These claims were checked and hold; they are recorded so the author knows they
were not overlooked:

- **kebab-case == per-field aliases for all config fields.** Every Python alias
  is multi-word (`scratch-containers`, `log-file`, `vault-addr`, `auth-user`,
  `auth-approle`, `auth-token`, `role-id`, `secret-id`); all single-word fields
  are kebab no-ops. `rename_all = "kebab-case"` reproduces them exactly
  (`config.py:35-180`).
- **Config optionality and shapes.** `storage`/`signing`/`logging`/`vault`
  optional with default `None`, `secrets` defaults to `[]`, `ccache` optional;
  `S3StorageConfig`, `S3LocationConfig`, `RegistryStorageConfig` shapes match
  (`config.py:103-180`). The `RegistryStorageConfig.url` FIXME is real
  (`config.py:131-137`).
- **`VaultConfig`** fields and aliases (`config.py:35-65`).
- **Secrets container** is four maps keyed by resource, with the third map named
  **`sign`** (not `signing`) — the design got this right (`models.py:359-365`,
  design line 125).
- **Vault auth order** AppRole → userpass → token (`vault.py:165-184`); **mount
  `ces-kv`** as a pinned constant (`vault.py:61`); **`read_secret`** returns
  `data.data` (`vault.py:71`); **Forbidden → permission-denied** mapping
  (`vault.py:65-66`).
- **No `schema-version` in Python.** Confirmed absent from `config.py` and
  `models.py`; the marker is a 001/002-mandated addition, not a fidelity gap.
- **"storage/signing/registry need no operator change."** True — they already
  carry `creds`/`type` on the wire (`models.py`); only git discriminates by
  shape.
- **Context-manager → RAII mapping.** `git_url_for` and `gpg_signing_key` are
  `@contextmanager` in Python (`mgr.py:68,78`); the other resolvers return plain
  tuples. The guard mapping is correct in principle (subject to F5 on the ssh
  scope).

## Required actions before proceeding

1. F2 — specify the per-family lookup contract (exact for storage,
   longest-prefix for git/registry) and the matcher, plus git's yield-unchanged
   no-match path.
2. F1 — correct `transit()` to `(mount, key)`.
3. F3 — re-model all families as single-level combined-discriminator types
   mirroring Python; drop the nested two-level framing.
4. F4 — re-justify or drop the git `type:` wire change against the
   custom-discriminator path; remove the "necessary" framing.
5. F5, F6 — tighten the ssh-guard scope and the signing field-shapes in the
   prose.
