# Adversarial design review — 012 static-musl acceptance & distro independence

- **Type:** design review (adversarial, single-doc, extension/clarification)
- **Target:**
  `design/012-20260623T2111-static-musl-acceptance-and-distro-independence.md`
- **Extends:** `design/001-…-architecture-and-implementation-spine.md`
  (build-target section + correctness invariant 7)
- **Date:** 2026-06-23
- **Reviewer mandate:** verify the technical soundness of the "static-musl ⇒
  distro-independent" argument, the accuracy of the kernel-ABI caveat, the
  supersession mechanics against the seq-docs-convention, internal consistency
  with 001/004/005/009/010, and completeness of the clarification for an
  implementer. Verified against the real repo
  (`container/ContainerFile.cbsd-rs`) and the whole design corpus, not just the
  docs 012 cites.

---

## VERDICT

**GO with one required pre-implementation fix (a doc edit).**

012 is correct on its core thesis and is a clean, well-scoped supersession. A
fully static-musl binary genuinely has no glibc-version axis, "oldest supported
distro" genuinely is glibc thinking that does not apply, and the link-time
staticness check (`ldd`/`file`) genuinely is the right primary gate. The
kernel-ABI caveat is accurate, the cross-references all resolve, and the
document leaves the approach, the runtime default distro, and the rest of 001
correctly intact.

The one real defect is an **overclaim by omission**: 012 equates "links fully
statically, no `openssl-sys` edge" with "distro-independent by construction,"
but a static binary that does **TLS** to S3 and Vault still needs a **runtime CA
trust source**, which is a userspace/distro-shaped dependency that pure
link-time staticness does not cover. This is not hypothetical for this codebase:
the existing `cbsd-rs-server` image installs `ca-certificates` with the in-repo
comment "needed by reqwest (rustls-tls) for TLS peer verification"
(`container/ContainerFile.cbsd-rs:138`) — direct evidence that the project's
rustls stack reads the **system** trust store, not bundled roots. The S3/Vault
calls run **inside the builder container** (`desc.distro`, e.g. `rockylinux:9`)
at C4+, an image cbscore does not control, so "distro-independent by
construction" is too strong without a CA-trust carve-out. This does not bite C0
(the probe only constructs clients; no live endpoint), so it is not a blocker
for the bootstrap phase, but the **claim** is what an implementer will carry
forward into 003/004/005, so it must be corrected in the doc before it is used
as the contract.

**Required before the doc is used as the implementation contract (doc edit
only):**

- **F1** — scope the distro-independence claim to the binary's
  _linkage/execution_ and explicitly carve out **TLS trust material**: a static
  binary's runtime TLS still needs a CA bundle (and the rustls trust source —
  `rustls-native-certs` vs bundled `webpki-roots` — must be pinned in
  003/004/005, not here). State that the C4+ in-container S3/Vault calls assume
  a CA bundle in the builder image (`desc.distro`), and cross-ref the design
  that pins it.

Nice-to-have (not blocking): F2 (note the syscall-floor caveat is bounded by the
controlled worker host), F3 (cross-ref 003/004/005 where the TLS stack is
actually selected), F4 (tighten the "any EL image is equivalent" wording so it
is not read as "no CA bundle needed").

---

## What was verified (claims that hold)

These were checked against the cited docs and the real repo and are **accurate**
— recorded so a future reader does not re-litigate them:

- **Forward pointers exist in 001 at both places.** 001's build-target section
  opens with a blockquote "Extended/refined by design 012…" (`001:69-76`) and
  the acceptance-gate bullet carries a parenthetical "_(Refined by 012…)_"
  (`001:102-106`); invariant 7 carries "_(Refined by design 012…)_"
  (`001:303-306`). 012's claim that "001 carries a forward pointer at both
  places" is true.
- **Runtime default `--distro` is rockylinux:9.** 010's `versions create` flag
  table lists `--distro | default=rockylinux:9 | kept` (`010:137`), and the
  runner spawns `desc.distro` (`009:69`, step 5). 012's "Rocky 9 remains the
  runtime default" is accurate, and 012 correctly refines only the _test_
  framing, not the default.
- **Nothing in 009 contradicts 012.** 009 spawns the image named by
  `desc.distro` and mounts the musl `cbsbuild` as PID 1; it never asserts Rocky
  9 is canonical. Consistent.
- **README index lists 012** with an accurate one-line summary (`README.md:69`).
- **Plan C0 derives acceptance from 012, not its own prose.** The plan's "Notes
  for the plan-review" says "**Acceptance is governed by design 012**" and the
  testable bullets split primary (link-time `ldd`/`file`) vs secondary (runtime
  EL smoke) exactly as 012 specifies
  (`plans/001-…-01-bootstrap.md:78-87, 143-151`). Consistent.
- **Kernel-ABI caveat is correct.** "Running a `rockylinux:9` container does not
  even exercise an old kernel (the container shares the host kernel)" is true —
  a container reuses the host kernel, so the EL9 _userspace_ image exercises no
  old syscall surface. This _supports_ 012's argument (it is why the EL run is a
  "not-actually-static" regression catch, not a portability proof) rather than
  undercutting it. See F2 for the one nuance.
- **Supersession scope is crisp.** 012 supersedes "the acceptance/verification
  wording" of one 001 section + invariant 7 and changes "nothing about the
  chosen approach." It does not touch crate layout, failure isolation, schema
  policy, the commit map, or any other invariant. This is a clean,
  convention-consistent supersession: 001 stays a snapshot, 012 adds forward
  pointers rather than retro-editing 001's substance, and the governing wording
  lives in exactly one place.

---

## Findings

### F1 — Overclaim: "distro-independent by construction" omits the runtime CA-trust dependency (required fix)

**Severity:** significant (doc-contract correctness; not a C0 blocker)

**Where:** 012 §"Refined acceptance" item 1 ("This is the real proof B1 needed —
that the heavy crates' TLS/crypto stacks resolve to a pure-Rust provider
(rustls + `ring`/`aws-lc-rs`) with **no** `openssl-sys` dynamic linkage");
§"Invariant 7, restated" ("links fully statically … which is the
distro-independent acceptance gate"); §"What 001 says…" ("distro-independent by
construction").

**Problem.** 012 conflates two distinct properties of a TLS-capable binary:

1. the **cipher/crypto provider** (rustls + ring/aws-lc-rs vs OpenSSL) — a
   _link-time_ property, and the thing the `ldd`/no-`openssl-sys` check actually
   proves; and
2. the **certificate trust source** — _where the binary gets the root CA set to
   validate a TLS peer_ — which is a **runtime** property.

Proving (1) says nothing about (2). With rustls, the trust source is a build
decision that 012 (and, more importantly, 004/005) never pins:

- `rustls-native-certs` reads the **host/container system trust store**
  (`/etc/ssl/certs/...`, `/etc/pki/tls/...`, the `ca-certificates` package) — a
  runtime userspace/distro dependency a fully static binary does **not** carry;
  or
- bundled `webpki-roots` compiles the Mozilla root set **into** the binary — no
  system dependency, but pinned-at-build-time and unable to honor an operator's
  custom/internal CA (relevant: cbscore talks to MinIO/RGW and an internal
  Vault, which are frequently fronted by a private CA).

The corpus picks neither. A grep of the entire `design/` tree for
`rustls|webpki|native-certs|crypto.provider|aws-lc|ca-cert|/etc/ssl|/etc/pki`
finds only the crypto-_provider_ mention in 012:47-48 and 001:99 — **the CA
trust source is specified nowhere.** 005 (the S3 client) requires only a
"musl-clean crypto provider (e.g. `rustls` with `ring`/`aws-lc-rs`)" and is
silent on trust; 004 (Vault) does not mention TLS material at all.

This is concrete for this repo, not pedantry. `container/ContainerFile.cbsd-rs`
installs `ca-certificates` in `worker-base` (`:52`) and in `cbsd-rs-server` with
the explicit comment "**needed by reqwest (rustls-tls) for TLS peer
verification**" (`:138`). That is in-repo proof that the project's current
rustls stack resolves trust from the **system** bundle, i.e. native-certs
behavior — exactly the runtime userspace dependency 012's "distro-independent by
construction" denies.

**Impact (and why it is not a C0 blocker but is a contract defect).**

- At **C0**, the musl probe only _constructs_ the S3/Vault clients (no live
  endpoint — `plans/001-…-01-bootstrap.md:103-106`), so no TLS handshake occurs
  and no trust store is consulted. F1 does **not** break C0. 012's acceptance
  gate is sound _for what C0 tests_.
- At **C4+**, the in-container `cbsbuild runner build` does real S3 uploads
  (signing → S3) and Vault reads. Per 009, that runs **inside the builder
  container** = `desc.distro` (e.g. `rockylinux:9`), an image **cbscore does not
  own**. If the build chose `rustls-native-certs` and a given `desc.distro` base
  ships no CA bundle (a stripped/minimal EL image), every S3/Vault TLS handshake
  fails at runtime — and 012 has told the reader the binary is
  "distro-independent by construction," which is precisely the assumption that
  hides this failure mode. EL base images usually do ship `/etc/pki`, so this is
  latent rather than guaranteed-broken, but the claim as written is what lets it
  slip through review.
- The danger of leaving the overclaim in the doc is that 012 is the _governing_
  wording for B1/invariant 7. An implementer reading "no `openssl-sys` edge ⇒
  distro-independent" will not think to pin the trust source or to guarantee a
  CA bundle in the builder image.

**Recommendation.** Edit 012 to:

1. Scope the distro-independence claim to **linkage and execution**: a
   static-musl binary has no libc-version axis and runs on any in-family kernel
   — true and unchanged. Then add a one-sentence **carve-out**: this covers the
   binary's _machine code_, not the **runtime TLS trust material** it consults
   when it talks to S3/Vault.
2. State explicitly that the C4+ in-container S3/Vault TLS calls require a **CA
   trust source**, and that _where_ it comes from — bundled `webpki-roots`
   (self-contained, but cannot honor a private CA) vs `rustls-native-certs`
   (reads the builder image's `/etc/pki|/etc/ssl`, reintroducing a userspace
   dependency on `desc.distro`) — is a decision **owned by 003/004/005**,
   not 012. If native-certs is chosen, note the builder image must ship a CA
   bundle (as `worker-base`/`cbsd-rs-server` already do for the host-side
   binaries).
3. Keep the link-time `ldd`/`file` gate exactly as is — it is correct; just stop
   it from carrying a claim it does not prove.

This is a **doc edit**, and it does not change the C0 acceptance gate.

---

### F2 — Kernel-ABI: the syscall-floor caveat is dismissed slightly too fast (minor)

**Severity:** minor (the conclusion is right; the framing has one unstated
bound)

**Where:** 012 §"What 001 says…" bullet 2 ("Whether a static binary runs depends
on the Linux **syscall** ABI, which is stable across distributions … A static
binary that runs on Alpine runs identically on Rocky, Alma, EL9, and EL10").

**Problem.** "Runs identically across distros" is true _at a fixed kernel_. The
Linux syscall ABI is stably **backward**-compatible (old syscalls keep working
on new kernels), but it is **not forward**-compatible: a musl binary built
against a newer kernel headers set can emit a syscall (or a syscall flag) that a
sufficiently old **host** kernel does not implement, yielding `ENOSYS` at
runtime. The variable is the **host kernel**, not the container userspace —
which is exactly 012's (correct) point that the EL9 _container_ tests nothing.
012 is right that distro is the wrong axis; it just never names the axis that
_does_ matter (host kernel version) or note why it is a non-issue here.

**Impact.** None in this deployment, but the doc leaves a reader who knows the
forward-compat subtlety wondering whether it was considered. In CBS the build
host (the worker running podman) is operator-controlled and not ancient, and the
musl toolchain targets a conservative baseline, so the syscall floor is
comfortably below any realistic host kernel. The gap is purely rhetorical.

**Recommendation.** Add half a sentence: the one portability axis a static
binary _does_ depend on is the **host** kernel's syscall floor (not the
container distro), which is a non-issue here because the worker host is
controlled and the musl target's syscall baseline sits well below it. This turns
a possible "did they think about ENOSYS?" into a closed question.

---

### F3 — Completeness: should cross-ref where the TLS stack is actually selected (minor)

**Severity:** minor (completeness for an implementer)

**Where:** 012 §"Refined acceptance" item 1 and §"Testing".

**Problem.** 012 asserts the crates "resolve to a pure-Rust provider (rustls +
`ring`/`aws-lc-rs`)" and that "the shipped graph gains no `openssl-sys` edge,"
but it does not point to the design that _owns_ that selection. 005 is the
S3-client design (and mentions the crypto provider in passing); 004 is the Vault
design; neither pins the provider crate, the feature flags, or the trust source
as a decision. So 012 makes a claim about a choice that no design actually
commits. Combined with F1, the reader has no doc that says "this is where
rustls + provider + trust source is pinned."

**Impact.** An implementer building the C0 probe (or the C4 clients) has to
re-derive the feature-flag set (`aws-sdk-s3` default TLS feature, `vaultrs` TLS
feature, `rustls` provider, `webpki-roots` vs `rustls-native-certs`) with no
authoritative pin. Two commits could pick different providers, defeating the "no
`openssl-sys` edge" invariant the C0 probe is supposed to lock in.

**Recommendation.** Have 012 cross-reference the design that pins the TLS stack
(likely 005 for S3, 004 for Vault) and, if none does today, note that the
provider + trust-source pin is a **prerequisite the C0 probe encodes** — the
probe's exact feature/provider selection becomes the canonical pin the shipped
C4/C6 manifests must match. (This dovetails with the plan's own "confirm the
musl crypto-provider choice … is pinned, not incidental" note at
`plans/001-…-01-bootstrap.md:140-142`, which today has no design to point at for
the _trust_ half.)

---

### F4 — Wording: "any EL image is equivalent for a static binary" can be misread (minor)

**Severity:** minor (wording, downstream of F1)

**Where:** 012 §"Refined acceptance" item 2 ("The image is arbitrary for a
static binary; use a representative EL image"); §"Invariant 7, restated" ("Rocky
/ Alma / EL9 / EL10 are equivalent for a static binary"); echoed in the plan
("any EL image is equivalent").

**Problem.** For the **C0 smoke test** the statement is exactly right — the
probe prints a version and exits, exercising no TLS, so any EL image (or none)
is equivalent. But the same "any EL image is equivalent" phrasing, read
alongside F1, invites the wrong generalization to the **C4+ runtime**, where the
images are _not_ equivalent if the trust source is native-certs (one ships a CA
bundle, a stripped one does not). The equivalence is true for _execution_ and
false for _TLS trust_ — the same conflation as F1.

**Impact.** Low on its own; it amplifies F1's overclaim. Fixing F1 mostly
resolves this.

**Recommendation.** Qualify the equivalence to the **smoke test's scope**:
"equivalent _for the version-print smoke test_ — the binary executes
identically; this says nothing about the CA bundle a real S3/Vault call needs
(see F1)."

---

## Confidence scoring

Scoring the **design document** as an artifact (clarity, correctness,
completeness, consistency, convention-adherence), per the confidence-scoring
rubric adapted to a design review.

| Item                                                                                                       | Points | Description                                                                                                                                  |
| ---------------------------------------------------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                                             | 100    |                                                                                                                                              |
| D8: spec/claim overreach — "distro-independent by construction" omits the runtime CA-trust dependency (F1) | -15    | Governing wording for B1/invariant 7 conflates link-time crypto provider with runtime trust; misleads the C4+ in-container S3/Vault contract |
| D11: missing the TLS-trust caveat / no cross-ref to where the stack is pinned (F3)                         | -5     | An implementer cannot find the authoritative provider+trust pin from 012                                                                     |
| D10: kernel-ABI framing omits the host-kernel syscall-floor bound (F2)                                     | -3     | Conclusion correct; one relevant axis left unnamed                                                                                           |
| D11: "any EL image is equivalent" wording over-generalizes past the smoke test (F4)                        | -2     | Minor, downstream of F1                                                                                                                      |
| **Total**                                                                                                  | **75** |                                                                                                                                              |

**Interpretation:** 75 — acceptable with the noted fix. The supersession
mechanics, cross-references, internal consistency, and the core static-musl
argument are all sound (no deductions there); the deductions are concentrated in
the single CA-trust overclaim (F1) and its two echoes (F3/F4) plus one framing
nuance (F2). Land F1 as a doc edit and this rises into the 90s.

---

## Findings ordered by severity

1. **F1 (significant, required fix)** — "distro-independent by construction"
   overclaims: a static-musl binary that does TLS to S3/Vault at C4+ (inside the
   `desc.distro` builder container) still needs a runtime CA trust source; the
   corpus pins the crypto provider but never the trust source, and the existing
   ContainerFile (`:138`) proves the project's rustls uses the **system** CA
   bundle. Scope the claim to linkage/execution and carve out TLS trust
   material; defer the trust-source pin to 003/004/005. Does **not** block C0.
2. **F3 (minor)** — 012 claims a rustls + ring/aws-lc-rs + no-`openssl-sys`
   stack but cross-references no design that owns that selection; pin it (likely
   005/004) or make the C0 probe the canonical pin.
3. **F2 (minor)** — the kernel-ABI argument is correct but never names the one
   axis that does matter (host-kernel syscall floor) or why it is a non-issue
   here.
4. **F4 (minor)** — "any EL image is equivalent for a static binary" is true
   only for the version-print smoke test; qualify it so it is not read as "no CA
   bundle needed."

**Verdict: GO**, conditional on the F1 doc edit before 012 is used as the B1
implementation contract. F2/F3/F4 are nice-to-haves that further harden the
clarification.
