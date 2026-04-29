# Decision 001: YAML format for CRT store configuration

**Date:** 2026-03-07
**Status:** Accepted

## Context

The CRT store needs a configuration file to define namespaces, channels,
and their defaults, replacing all hardcoded `["ces", "ccs"]` references.

## Decision

Use YAML (`crt.config.yaml`) at the repository root, as specified in the
implementation plan. This requires adding `pyyaml` and `types-pyyaml` as
dependencies to the `crt` package.

## Rationale

- YAML is the format specified in the implementation plan
- More human-readable than JSON for hierarchical config with descriptions
- Python's stdlib `tomllib` (3.11+) was considered but YAML better fits the
  nested namespace/channel/branding structure
- Type stubs (`types-pyyaml`) ensure basedpyright strict mode compatibility

## Consequences

- Two new runtime dependencies: `pyyaml>=6.0`, `types-pyyaml>=6.0`
- Config errors integrate with existing `CRTError` hierarchy via
  `errors/config.py`
