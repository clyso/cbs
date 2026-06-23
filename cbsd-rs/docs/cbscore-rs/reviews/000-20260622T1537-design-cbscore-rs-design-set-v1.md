# Whole-set adversarial design review — cbscore-rs design corpus

- **Type:** design review (adversarial, cross-set / whole-corpus pass)
- **Scope:** the eleven subsystem designs (001–011) plus the two new,
  previously-unreviewed top-level docs (`README.md`, `CLAUDE.md`), `ROADMAP.md`,
  and the baseline reference review (`000` at the top level). Per-doc reviews in
  `reviews/` consulted as settled context.
- **Date:** 2026-06-22
- **Reviewer mandate:** cross-reference integrity, internal consistency across
  docs, README/CLAUDE accuracy against design 004 and the Python source,
  completeness of the set against `cbscore/src/cbscore/`, and
  capability-commit-map sanity. NOT a re-derivation of each design.

## Method

Each new claim was checked against the authoritative design it cites and, where
coherence depends on it, against the Python source (`cbscore/src/cbscore/`).
Cross-references (`owned by 0NN`, `resolves Bn/Hn/Mn`, capability tags `Cn`)
were spot-checked for resolution and accuracy. The README operator
config/secrets/vault reference was checked field-by-field against design 004 and
its sources (`config.py`, `utils/secrets/models.py`, `utils/secrets/signing.py`,
`utils/vault.py`).

---

## VERDICT

**GO for the set, with two pre-implementation fixes.** The corpus hangs together
as one coherent specification: the eleven designs cross-cite each other
accurately on the substance (ownership, types, error variants, defaults), the
two new top-level docs (README, CLAUDE) are faithful renderings of 004 and the
spine, the Python surface is fully covered, and the capability map is honestly
sliced by capability with the one scaffolding bucket (C0) explicitly justified.
The findings are concentrated in **two classes that do not change any design's
behavior**: (1) a cluster of wrong baseline-finding letter citations
(B-letter/H-/M-) that send a reader to the wrong 000 entry, and (2) one
genuinely operator-misleading inaccuracy in the README's secrets reference
(transit `key` semantics). Both should be corrected before the docs are used as
the implementation contract, but neither blocks the GO.

**Required before implementation (doc edits only):**

- **Fix B1** — correct the README's transit `key` description and the
  `# read from ces-kv` example comment; transit `key`/`mount` are not ces-kv
  reads.
- **Fix the citation cluster (A5, A6; A1, A2 if cheap)** — 005's
  `list_releases`→"M3" and 006's title→"H3" citations point at the wrong 000
  findings; correct or drop the letters.

Nice-to-have: A2 (name the parent `cbsd-rs/CLAUDE.md` for invariant #4), C1
(mention the `advanced` drop).

---

## Findings

### Category (a) — cross-set consistency / broken references

**Baseline-finding resolution map (forward direction — every 000 blocker / HIGH
/ MEDIUM has an owning design).** Each baseline finding is in fact resolved
somewhere in the set; verified by reading the cited resolution:

| 000 | Finding                          | Resolved by                  |
| --- | -------------------------------- | ---------------------------- |
| B1  | binary mount / build target      | 001 (musl) + 009 (mount)     |
| B2  | crash containment / panic policy | 001 + 011 (`is_panic` task)  |
| H1  | build-report round-trip          | 009 (invariant 5)            |
| H2  | `runner build` CLI contract      | 009 + 010 (one shape)        |
| H3  | `CBS_DEBUG` value forwarding     | 009 + 010 (`-e CBS_DEBUG=…`) |
| H4  | S3/MinIO parity                  | 005 (invariant 9)            |
| H5  | Vault auth order / `ces-kv`      | 004 (invariant 8)            |
| M1  | CLI parity enumeration           | 010 (flag table)             |
| M2  | schema-version bump policy       | 001 + 002 (pragmatic)        |
| M3  | `get_version_type` is a lookup   | 006                          |

(Note: invariant 4 / secret redaction has **no** baseline letter — it is a port
improvement stricter than Python, not a 000 gap; A1 below is about 001
mis-attributing it to H4, not a missing resolution.)

- **A1 (001 H4 mis-citation).** 001 invariant 4 (secret redaction) says
  "resolves review H4's leakage class". 000's H4 is the S3/MinIO parity finding
  (credentials source + endpoint + addressing) — it contains no
  redaction/leakage content. Secret redaction was not assigned a letter in 000.
  001 invariant 9 also cites H4 (correctly, for S3 addressing). So H4 is cited
  twice and one of the two (invariant 4) is a mis-reference. Low severity (does
  not change behavior) but it is a broken cross-reference in the spine that the
  README/CLAUDE inherit.

- **A2 (011 "CLAUDE.md invariant #4" is the wrong CLAUDE / wrong topic).** 011
  says it "updates / amends **CLAUDE.md correctness invariant #4**" for the
  `trace_id` lifecycle (lines 94-106, 303-305). In the **new
  `cbscore-rs/CLAUDE.md`** (the one in this doc set), invariant #4 is **Secret
  redaction** (`SecureArg`), not `trace_id`. The invariant 011 actually amends
  is `cbsd-rs/CLAUDE.md`'s #4 ("`trace_id` lifecycle"). Now that two `CLAUDE.md`
  files each carry a numbered invariant list, the bare "CLAUDE.md correctness
  invariant #4" resolves to the wrong topic for a reader of the cbscore-rs set.
  The reference is correct only if read as the parent workspace file; it should
  name it explicitly. LOW (cross-reference ambiguity, no behavioral impact).

- **A3 (timeout defaults — VERIFIED CONSISTENT, no finding).** The brief flagged
  009 CLI "4 h" vs 011 worker "7200 s". This is **deliberately a two-layer
  default and is reconciled**: 011 (lines 324-328, 366-367) states the worker
  supplies `RunOpts.timeout` defaulting to 7200 s (preserving the wrapper's
  `CBS_BUILD_TIMEOUT`) and "never falls through to 009's 4 h default (which now
  governs only the CLI's no-override path)." 009 (lines 171-172) confirms 4 h is
  the CLI/Python-parity default, overridable per build. Not a contradiction.

- **A4 (RunnerError variants — VERIFIED CONSISTENT).** 009's `RunnerError`
  variants (`NonZeroExit { report, stderr }`, `Podman(PodmanError)`,
  `Cancelled`, plus marshalling errors) match 011's consumption table (lines
  116-121) exactly, including which variant carries a report. Panic isolation
  (`JoinError::is_panic()` + pinned `panic = "unwind"`, no `catch_unwind`) is
  consistent across 001, 011, README decision 2, and CLAUDE decision 2. No
  finding.

- **A5 (005 misattributes the `list_releases` arity bug to "review M3").** 005
  cites the broken `list_releases` call site as "review M3" (line 119) and
  "(M3)" (line 140). 000's **M3 is `get_version_type` misclassified as a regex
  parser** — not `list_releases`. The `list_releases` arity bug is not a
  lettered 000 finding at all; it is a separate Python source defect (the
  "broken Python the port fixes" class in the README, attributed there to 006).
  001 (lines 222-229) also presents it as its own bug, without an M-letter. Two
  wrong citations in 005. MEDIUM (broken cross-reference that a reader will
  follow to the wrong baseline finding).

- **A6 (006 misattributes the title/description fix to "H3" — twice).** 006 says
  the title uses "General Availability" not "Release" and tags this "resolving
  000's H3" (line 60) and "(000's H3)" (fidelity note, line 226). 000's **H3 is
  the `CBS_DEBUG` flag-value-drop bug**, not anything about version titles or
  the type-description column. The title/`get_version_type_desc` behavior maps
  to **no lettered 000 finding** at all (it is not in 000's deduction table).
  009 and 010 correctly cite H3 for `CBS_DEBUG`. So 006's two H3 citations point
  a reader at the wrong baseline finding. MEDIUM.

- **A7 (capability tags — spot-checked, consistent).** Capability tags in prose
  match 001's commit map on the points checked: 005 tags S3 write/release ops
  `(C4)`/`(C6)` and listing `(C7)`; 006 tags `versions list` `(C7)` and the
  configurable store `M5/C9`; 003 tags first consumers `(C1)`/`(C3)`/`(C6)`;
  009/011 use `C2`/`C8` consistently with 001's index. No mismatch found. (See
  D-category for the one map-level judgment.)

### Category (b) — README / CLAUDE accuracy

- **B1 (README transit `key` semantics inaccurate).** README line 183 states,
  for the secrets file: "a `vault` entry additionally carries `key` (the
  `ces-kv` path to read)." This is a blanket claim over all vault variants. It
  is correct for git-vault, storage-vault and the GPG-vault variants
  (`signing.py:53` reads `vault.read_secret(entry.key)` from `ces-kv`). It is
  **wrong for the `transit` family**: `signing_transit` returns
  `(secret.mount, secret.key)` directly to the transit sign operation
  (`signing.py:171-184`) and never reads `ces-kv`. For transit, `key` is the
  **Vault transit key name** (used as `--key=hashivault://<key>`, 008:170) and
  `mount` is the **transit engine mount** (`TRANSIT_SECRET_ENGINE_PATH`,
  008:184) — neither is a `ces-kv` KV path. The error is compounded in the
  worked example: the `ces-transit-key` entry's `key: transit/ces` carries the
  inline comment **`# read from ces-kv`** (README line 235), which is exactly
  the wrong mental model — the transit `key` is never read from `ces-kv`. An
  operator config reference must get this right; as written it misdescribes the
  one cross-cutting field. (004's "transit returns (mount, key)" is correct and
  does not make the ces-kv claim; the inaccuracy is the README's general
  sentence + the example comment.) MEDIUM.

- **B2 (README transit field-shape summary omits `key`; minor).** README line
  252 lists `transit (vault) {mount}`, mirroring 004. The Python
  `VaultTransitSecret` carries `creds`, `key` (inherited from `VaultSecret`),
  `type: transit`, and `mount`. The README's worked example (lines 232-235)
  correctly shows both `key` and `mount`, and the parenthetical at line 245 says
  "`vault` variants add `key`", so the summary is technically reconcilable.
  Cross-references B1 (the description of what `key` MEANS for transit is the
  real issue). LOW.

- **B3 (README/CLAUDE invariants & decisions vs 001 — VERIFIED CONSISTENT).**
  CLAUDE's 10 correctness invariants match 001's 10 one-for-one (wording differs
  only where CLAUDE is more precise, e.g. invariant 4's "the trait has no
  `Debug` supertrait", which 003 backs; and CLAUDE wisely drops the inherited H4
  mis-citation). The 7 settled decisions in README and CLAUDE are identical and
  match the set (reimplement-fresh,
  `JoinError::is_panic()`/`panic = "unwind"`/no async Drop, static-musl
  mount-by-path, pragmatic schema policy, `config init` dropped, git `type:`,
  UUIDv7 from M1). README's capability/milestone summary (M0 C0 → M5 C9, M2
  decomposed C2→C6) matches 001's commit map exactly. The README design-index
  "Owns" column was checked row-by-row against each design's stated ownership —
  all accurate. No finding.

- **B4 (README config reference vs 004 + Python — VERIFIED ACCURATE except
  B1).** Field names (kebab-case), the four secret families and their
  `(creds, type)` discriminators, the git `type:` field (plain-only `token`;
  ssh/https plain-or-vault), the Vault auth order (AppRole→userpass→token) and
  `ces-kv` mount, and `schema-version: absent→1` all match 004 and the Python
  sources (`config.py`, `secrets/models.py`, `vault.py`, `signing.py:171-184`).
  The git/storage field shapes, the `signing.gpg`/`signing.transit` "secret IDs"
  note, and the exact-vs-longest-prefix lookup distinction are correct. The
  worked examples parse against the Python models. Only B1 (transit `key`
  semantics) is inaccurate.

### Category (c) — completeness gaps

- **C-cov (corpus coverage — COMPLETE, no gap).** Every Python module under
  `cbscore/src/cbscore/` maps to an owning design: `builder/`→007 (+report
  type→002); `containers/`→008; `core/component.py`→007; `images/{desc}`→006,
  `{skopeo}`→003, `{signing,sync}`→008; `releases/{desc}`→002, `{s3}`→005,
  `{utils}`→007; `runner.py`→009; `utils/{git,podman,buildah}`→003, `{s3}`→005,
  `{containers,uris helpers}`→008, `{vault}`→004, `{secrets/*}`→004;
  `versions/`→006 (+desc→002); `config.py`→004; `cmds/`→010 (`config`/`advanced`
  intentionally dropped). `utils/uris.py:matches_uri` is covered by 004's
  longest-prefix matching (cited at 004:194); `utils/paths.py`
  (`get_script_path`) is covered behaviorally by 007's component-script
  contract. No subsystem is un-designed and no C0–C9 capability lacks a design
  home. No finding.

- **C1 (CLAUDE/001 invariant-2 enumeration omits the `advanced` drop —
  trivial).** 010 drops two command groups (`config`, `advanced`); 001 invariant
  2 and CLAUDE invariant 2 name only the `config` group among command-level
  drops. `advanced` is an empty hidden group (nothing to port —
  `advanced.py:22-24`), so this is not a functional gap, but the enumeration is
  incomplete relative to 010. LOW / informational.

### Category (d) — capability-map issues

- **D1 (C0 is a de-risk spike, not a capability — acceptable, by the doc's own
  framing).** Per the git-commits smell test, a commit should deliver a testable
  capability and ship no dead code. C0 ships "workspace
  - 3 crate skeletons" plus a musl-linkage CI proof, which is closer to
    scaffolding than a user/operator capability. 001 (lines 190-196) pre-empts
    this honestly: C0 is "an explicit one-time **de-risk spike** for B1, not a
    feature commit", its testable artifact is "a static binary runs as PID 1 in
    EL9 (prints version)", and crucially the `aws-sdk-s3`/`vaultrs` musl proof
    "lives in a CI job … **not** as a shipped dependency edge" — those crates
    enter the manifest only at the commits that first use them (S3 at C4/C6,
    Vault at C4), "so C0 carries no dead dependency." This is the right call:
    the bootstrap genuinely must exist before any capability and it has a
    concrete acceptance test. Not a finding; noted as the one bucket that is
    scaffolding-shaped, and it is justified.

- **D2 (C2 is a large keystone — flagged and pre-split by the map itself).** C2
  ("build end-to-end, keystone") lands the host runner + config load/store +
  secrets-file load/store + components aggregation + config rewrite + podman
  wrapper + report round-trip + `Builder` skeleton + the `build`/`runner build`
  CLI in one bucket — clearly over the ~800-line budget. 001 (lines 243-249)
  anticipates this: each milestone "becomes a capability plan … pins each commit
  to ~400–800 authored lines and presents that breakdown for approval", and "C2
  is expected to split there along capability seams (container mechanism
  → +`prepare_builder`)", with the splits required to stay capability-based. The
  map is therefore an honest milestone map, not the final commit plan; the
  sizing risk is deferred to the per-milestone plans with an explicit
  anti-layer-split constraint. Acceptable for a design-level map; the actual
  sizing discipline must be enforced at plan time. LOW (a watch-item for the
  plans, not a defect in the design set).

- **D3 (capability slicing is otherwise sound).** C1, C3, C4, C5, C6, C7, C8
  each deliver a distinct, testable increment ("create a descriptor", "compile a
  component's RPMs", "sign + upload", "push image", "full parity
  reuse/skip/transit", "list releases", "worker runs in-process"). Foundational
  code is consistently placed "in the commit of its first consumer" with a
  "lands here" column as the no-dead-code evidence — the explicit antidote to
  the layer-by-layer anti-pattern 000 flagged. No finding.

---

## Findings ordered by severity

| Sev    | ID  | Finding                                                           |
| ------ | --- | ----------------------------------------------------------------- |
| MEDIUM | B1  | README transit `key` described as a `ces-kv` path; it is not      |
| MEDIUM | A5  | 005 cites the `list_releases` arity bug as "M3" (M3 ≠ that bug)   |
| MEDIUM | A6  | 006 cites the title/description fix as "H3" (H3 = `CBS_DEBUG`)    |
| LOW    | A1  | 001 invariant 4 cites "H4's leakage class"; H4 is S3 parity only  |
| LOW    | A2  | 011 "CLAUDE.md invariant #4" resolves to the wrong CLAUDE/topic   |
| LOW    | B2  | README transit field-shape summary omits `key` (example is right) |
| LOW    | C1  | 001/CLAUDE invariant 2 enumeration omits the `advanced` drop      |
| INFO   | D1  | C0 is a scaffolding-shaped de-risk spike (explicitly justified)   |
| INFO   | D2  | C2 keystone exceeds the line budget (pre-split deferred to plans) |

**Verified clean (not findings), recorded so they are not re-litigated:** A3
(timeout 4 h vs 7200 s is a deliberate two-layer default, reconciled), A4
(`RunnerError` variants 009↔011 + panic policy consistent), A7 (capability tags
in prose match 001), B3 (README/CLAUDE invariants, decisions, milestone summary,
and design-index "Owns" column all match 001), B4 (README config/secrets/vault
reference matches 004 and the Python sources except B1), C-cov (full
Python-surface coverage), D3 (capability slicing sound).

## Confidence score

Scoring the **design set's coherence as a specification** (not any one design's
internal soundness — those were scored in `reviews/`). Start at 100; deduct per
distinct cross-set defect by severity. The two per-doc-review multi-version
designs (006/009/011) reached high scores in their latest reviews and are
treated as settled; deductions here are only for cross-set / new-doc defects.

| Item                                                      | Points |
| --------------------------------------------------------- | ------ |
| Starting score                                            | 100    |
| B1 — README transit `key` operator-misleading (D11)       | −7     |
| A5 — 005 wrong "M3" citation (broken cross-ref, ×2) (D10) | −5     |
| A6 — 006 wrong "H3" citation (broken cross-ref, ×2) (D10) | −5     |
| A1 — 001 invariant-4 H4 mis-citation (D10)                | −3     |
| A2 — 011 ambiguous "CLAUDE.md invariant #4" (D10)         | −2     |
| B2 — README transit shape summary omits `key` (D11)       | −2     |
| C1 — invariant-2 omits `advanced` drop (D10)              | −2     |
| **Total**                                                 | **74** |

74/100 — upper "significant issues; address before proceeding" band, one point
under "acceptable with noted improvements". The score is dominated by a
citation-hygiene cluster (A1/A2/A5/A6 = −15 combined) that is mechanical to fix
and changes no behavior, plus the one substantive README inaccuracy (B1). With
B1, A5, and A6 corrected the set clears comfortably into the merge-ready band.
The substance of the corpus — types, error variants, defaults, ownership,
coverage, and the capability map — is consistent and complete; the defects are
in the connective tissue (which baseline finding a fix resolves) and one
operator-facing sentence, not in the designs' content.

## Scope honesty

This pass verified cross-references, internal consistency, README/CLAUDE
accuracy against 004 + Python, Python-surface coverage, and capability-map
slicing. It did **not** re-derive each subsystem's fidelity to Python (that is
the per-doc reviews' job and was treated as settled), and it did not
exhaustively diff every capability tag in every design (a representative
spot-check found them consistent — A7). The golden-file / round-trip / parity
**tests** each design specifies are design-level promises; whether the
implementation honors them is for `phase-review` after each milestone, not this
design pass.
