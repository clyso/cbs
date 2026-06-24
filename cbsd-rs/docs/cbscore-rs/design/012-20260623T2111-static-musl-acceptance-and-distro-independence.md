# 012 — Static-musl acceptance & distro independence (extends 001)

This document **extends and refines design 001**, specifically its "Build target
& portability (resolves review B1)" section and correctness invariant 7. It
changes **nothing** about the chosen approach — a static-musl `cbsbuild` mounted
by explicit path into the builder container — and narrows only **how B1 is
accepted and verified**. Design 001 remains the spine; this is a focused
clarification of one acceptance gate. Read 001 first; 001 carries a forward
pointer to this document at the two affected places.

## What 001 says, and the imprecision

001 frames the B1 acceptance gate as: "the static `cbsbuild` runs as PID 1 in
the **oldest supported** `desc.distro` (rockylinux:9)." Invariant 7 repeats it:
"the static-musl `cbsbuild` runs as PID 1 in the oldest supported `desc.distro`
(rockylinux:9)."

That wording carries a **glibc forward-compatibility assumption that does not
apply to the static-musl approach 001 itself chose**:

- **"Oldest supported distro" is glibc thinking.** Building against the oldest
  glibc to maximise forward-compat is how you ship a _dynamically-linked glibc_
  binary. A **fully static musl** binary has **no** libc linkage at all: it
  carries its own (musl) libc and issues raw Linux syscalls, so there is no
  glibc symbol version to be compatible with and "oldest" is moot.
- **Running is a kernel-ABI property, not a distro property.** Whether a static
  binary runs depends on the Linux **syscall** ABI, which is stable across
  distributions — not on the userspace it lands in. A static binary that runs on
  Alpine runs identically on Rocky, Alma, EL9, and EL10. The one axis that _can_
  matter is the **host-kernel syscall floor** (a binary assuming a newer syscall
  failing on an older kernel) — a non-issue here: the worker host is controlled
  and modern and musl targets are conservative. Running a `rockylinux:9`
  **container** does not even exercise that axis (the container shares the host
  kernel), so it tests nothing distro-specific.
- **cbscore is not Rocky-9-specific.** `desc.distro` is per-build; builds run on
  multiple EL versions (el9, el10) and EL-family distros (Rocky, Alma). Baking
  "rockylinux:9" into the acceptance gate as **the** supported distro overstates
  its role and misleads a reader into thinking the binary is validated for one
  distro rather than being distro-independent **in its linkage and execution**
  (the one genuine runtime-userspace dependency — TLS trust — is carved out
  below).

## Refined acceptance (the operative gate)

B1 acceptance is a **primary, distro-independent gate** plus a **secondary smoke
test**:

1. **Primary — link-time staticness (distro-independent).** The release
   `cbsbuild` (and the C0 musl probe that links `aws-sdk-s3` + `vaultrs`) links
   **fully statically**: `ldd` reports "not a dynamic executable" and `file`
   reports "statically linked". This is the real proof B1 needed — that the
   heavy crates' TLS/crypto stacks resolve to a pure-Rust provider (rustls +
   `ring`/`aws-lc-rs`) with **no** `openssl-sys` dynamic linkage. It is
   verifiable on the Alpine build host with no EL image at all.
2. **Secondary — runtime smoke (representative, not canonical).** The static
   binary is executed in a **glibc EL builder container** to catch a "not
   actually static / still wants an ELF interpreter" regression that a musl host
   would hide. For this **version-print** smoke (no TLS, no network) the image
   is arbitrary; use a representative EL image (Rocky 9 as the current default,
   or equivalently Alma / EL10), and running **el9 and el10** makes the
   distro-independence of _execution_ explicit. "Equivalent" here means **for
   execution only** — real S3/Vault TLS at C4+ adds the CA-trust dependency in
   the carve-out below.

## Carve-out: TLS trust is a runtime userspace dependency

The "distro-independent" claim is precise about **linkage and execution** — the
static-musl binary loads and runs on any EL kernel without a libc. It does
**not** extend to one runtime concern: **TLS trust**. `cbsbuild` opens TLS
connections to S3 (`aws-sdk-s3`, the upload/read path) and Vault (`vaultrs`,
secret resolution) during the in-container build (C4+), and TLS peer
verification needs a **CA trust source at runtime**:

- The corpus pins the crypto **provider** (rustls + `ring`/`aws-lc-rs`, no
  `openssl-sys`) but not the **trust source**. The existing cbsd-rs stack uses
  rustls reading the **system trust store** (`container/ContainerFile.cbsd-rs`
  installs `ca-certificates` "for TLS peer verification"), so by default the
  binary depends on its environment providing CA certificates under `/etc/ssl` /
  `/etc/pki`.
- Those TLS calls run **inside the builder container** (`desc.distro`, e.g.
  `rockylinux:9`) — an image cbscore does **not** own. So the builder image (or
  `prepare_builder`, 007) must ensure `ca-certificates` is present, and an
  operator with an **internal CA** (a private Vault / MinIO / RGW endpoint — the
  likely deployment) must have that CA in the trust store. This is a genuine
  runtime-userspace dependency, distinct from static linkage.
- The **trust-source decision** (system store via `rustls-native-certs` vs a
  bundled `webpki-roots`) belongs to the S3 and Vault client designs (005, 004),
  not here. Internal-CA support argues for the **system store**; 012 only flags
  that the choice exists and that "static" does not make TLS self-contained.

This carve-out does **not** affect the **C0** gate: the C0 probe only
_constructs_ the clients (no live handshake — plan C0), so it exercises only
linkage, which is genuinely distro-independent. The CA-trust dependency first
bites at C4+ (real S3/Vault calls), where 004/005/007 own it.

## What is unchanged

- The **approach** — static-musl `cbsbuild`, built `x86_64-unknown-linux-musl`,
  mounted by explicit path into the builder and run as PID 1 (001 B1 / 009) — is
  unchanged.
- **Rocky 9 remains the runtime default** `--distro` / `desc.distro` (design
  010). This document refines the _test_ framing only, not the default builder
  image.
- Every other part of 001 (crate layout, failure isolation, schema policy, the
  capability commit map) stands as written.

## Invariant 7, restated

> **Binary portability (refined).** The `cbsbuild` binary links **fully
> statically** against musl — `ldd`/`file` confirm no dynamic dependency — which
> is the distro-independent acceptance gate for its **linkage and execution**. A
> secondary runtime smoke run in a representative glibc EL container (Rocky /
> Alma / EL9 / EL10 are equivalent for _execution_) confirms it runs. It is
> **not** pinned to "the oldest supported distro" — for a static-musl binary
> there is no oldest-glibc axis to be compatible with. Its **TLS trust** to
> S3/Vault is a separate runtime-userspace dependency (see the carve-out), owned
> by 004/005.

## Supersession

This document **supersedes the acceptance/verification wording** of design 001's
"Build target & portability (resolves review B1)" section and correctness
invariant 7. Where 001 says "runs as PID 1 in the oldest supported `desc.distro`
(rockylinux:9)", the refined acceptance above governs. 001 carries a forward
pointer at both places. The implementation plan (`plans/001-…-01-bootstrap.md`,
C0) derives its acceptance criteria from **this** document, not from its own
prose.

## Testing

- **Link-time:** CI (the new push/PR workflow, plan C0) asserts the musl probe
  and `cbsbuild` are fully static (`ldd` → "not a dynamic executable"); the
  shipped graph gains no `openssl-sys` edge.
- **Runtime:** the static binary smoke-runs and prints its version inside a
  glibc EL container; running both an el9 and an el10 image demonstrates the
  _execution_ outcome does not depend on the distro.
- **TLS trust (not a C0 gate):** the C0 probe does no handshake, so C0 proves
  linkage only. Real S3/Vault TLS (C4+) requires a CA trust source in the
  builder container; that is verified where 004/005/007 own it, not here.
