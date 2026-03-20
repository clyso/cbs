---
name: cbsd-rs-docs
description: >
  Use when creating, naming, or placing design documents, plans, or
  review documents for the cbc or cbsd-rs packages in the cbs.git
  repository. Covers directory routing, file name format, sequence
  numbering, and timestamp conventions for cbsd-rs/docs/.
---

# cbsd-rs-docs — Document Storage & Naming

## Directory Structure

All documentation lives under `cbsd-rs/docs/` (repo root–relative):

```
cbsd-rs/docs/
  cbc/
    design/
    plans/
    reviews/
  cbsd-rs/
    design/
    plans/
    reviews/
```

**Package routing:**

- `cbc` package → `cbsd-rs/docs/cbc/`
- All cbsd-rs workspace packages (`cbsd-proto`, `cbsd-server`,
  `cbsd-worker`) → `cbsd-rs/docs/cbsd-rs/`

Each package directory maintains its own **independent** sequence
namespace.

## File Name Format

### Design documents

```
<seq>-<timestamp>-<title>.md
```

### Plan documents

```
<seq>-<timestamp>[-<sub>]-<title>.md
```

The optional `<sub>` (2-digit: `01`–`99`) is used when a single design
spawns more than one plan file. Omit it when there is only one plan.

### Review documents

```
<seq>-<timestamp>-<type>-<title>-v<N>.md
```

| Part | Format | Description |
|------|--------|-------------|
| `seq` | `001`–`999` | 3-digit zero-padded; from design docs, shared by related plans/reviews |
| `timestamp` | `YYYYMMDDTHHmm` | Current time when writing the document |
| `sub` | `01`–`99` | Plans only; 2-digit counter for multi-part plans |
| `type` | `design`\|`plan`\|`impl` | Reviews only; what is being reviewed |
| `title` | kebab-case | Short descriptive title; reviews mirror their design's title |
| `vN` | `v1`–`v99` | Reviews only; iteration counter, **not** zero-padded |

### Exempt files

`README.md`, `deployment.md`, and other operational or index files in
the `plans/` directory are not subject to the naming convention.

## Sequence Numbers

- `seq` is assigned by design documents only.
- Plans and reviews that relate to a design **share** that design's
  `seq`.
- To find the next `seq`: scan all files under
  `cbsd-rs/docs/<package>/design/`, extract the highest numeric prefix,
  and add 1. Start at `001` if no design files exist yet.

## Timestamp

Use the current local time at the moment of writing:

```
strftime format: %Y%m%dT%H%M
example:         20260320T0945
```

## Examples

```
# Design document (seq 009, cbsd-rs package):
cbsd-rs/docs/cbsd-rs/design/
  009-20260320T0750-dev-oauth-bypass.md

# Single plan for that design (same seq, no sub):
cbsd-rs/docs/cbsd-rs/plans/
  009-20260320T0800-dev-oauth-bypass.md

# Multi-part plans for seq 002:
cbsd-rs/docs/cbsd-rs/plans/
  002-20260318T1411-01-foundation.md
  002-20260318T1411-02-permissions-builds.md
  002-20260318T1411-03-dispatch-logs.md

# First design review (type=design, v1):
cbsd-rs/docs/cbsd-rs/reviews/
  009-20260320T0741-design-dev-oauth-bypass-v1.md

# Second iteration of the same design review:
cbsd-rs/docs/cbsd-rs/reviews/
  009-20260320T0750-design-dev-oauth-bypass-v2.md

# First plan review:
cbsd-rs/docs/cbsd-rs/reviews/
  009-20260320T0758-plan-dev-oauth-bypass-v1.md

# cbc design — cbc has its own independent seq (next: 007):
cbsd-rs/docs/cbc/design/
  007-20260320T1000-streaming-logs.md
```

## Current Sequence State

As of the skill's creation (2026-03-20):

- **`cbsd-rs/docs/cbsd-rs/`** — designs run `001`–`009`. Next seq:
  **010**.
- **`cbsd-rs/docs/cbc/`** — existing designs use 2-digit seq `00`–`06`
  (pre-canonical). Next seq: **007** (3-digit going forward).
