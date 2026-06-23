// CRT core — release-notes rendering (minijinja, deterministic).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Render release notes from a sealed [`Manifest`] (design §7.2). Pure: the
//! template bytes are passed in (the `crt` binary fetches them from the store by
//! the sealed `RenderSpec.template_digest`), so this crate stays IO-free.
//!
//! Three inputs are pinned so a re-render is reproducible: the branding snapshot
//! (in the manifest), the template (by digest), and the `minijinja` version
//! ([`RENDER_MINIJINJA_VERSION`], gated by [`check_render_version`]). A *public
//! projection* of the manifest is exposed to the template via serde — with
//! `justification.internal` stripped (see [`render_notes`]) — so templates
//! address fields by their serde names (`branding.display_name`,
//! `entry.justification.public_summary`, …) but can never reach the internal
//! note, regardless of what the template references.

use minijinja::{Environment, Value};

use crate::{CrtCoreError, Manifest, RenderSpec};

/// The `minijinja` version this build links — stamped into `RenderSpec` at seal
/// and checked at render/verify. The single source of truth (like
/// [`crate::SCHEMA_VERSION`]); kept in lockstep with the **exact** pin in
/// `crt-core/Cargo.toml`. Exact match is intentional (design §7.2: a version
/// mismatch errors rather than silently re-rendering with a different engine).
pub const RENDER_MINIJINJA_VERSION: &str = "2.21.0";

/// Render `manifest` through `template` (a minijinja template string).
/// Deterministic: no clock, no RNG; group/iteration order is fixed by the
/// manifest's entry order and minijinja's `groupby`.
///
/// `justification.internal` is stripped before the manifest reaches the template
/// context, so internal-hiding is *structural* — no template (or future caller)
/// can leak the downstream-only note, rather than relying on the default
/// template happening not to reference it.
pub fn render_notes(manifest: &Manifest, template: &str) -> Result<String, CrtCoreError> {
    // Redact the downstream-only `internal` notes into a public projection. The
    // projection carries every other field unchanged, so templates are otherwise
    // unaffected; the clone is cheap relative to a single-shot render.
    let mut public = manifest.clone();
    for entry in &mut public.entries {
        entry.justification.internal = None;
    }

    let mut env = Environment::new();
    // Jinja2-style whitespace control so control-flow lines (`{% for %}`,
    // `{% if %}`) don't each leave a blank line — lets the template be authored
    // in natural Markdown. Deterministic regardless of this setting.
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.render_named_str("release-notes", template, Value::from_serialize(&public))
        .map_err(|e| CrtCoreError::Render(format!("{e:#}")))
}

/// Guard that a sealed `spec`'s pinned `minijinja` version matches the one this
/// build links (design §7.2). A mismatch is an error: the re-rendered bytes
/// could differ from the sealed/materialized notes, so we refuse rather than
/// mislead.
pub fn check_render_version(spec: &RenderSpec) -> Result<(), CrtCoreError> {
    if spec.minijinja_version == RENDER_MINIJINJA_VERSION {
        Ok(())
    } else {
        Err(CrtCoreError::RenderVersionMismatch {
            sealed: spec.minijinja_version.clone(),
            linked: RENDER_MINIJINJA_VERSION.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Blast, Branding, Conflict, Coverage, Identity, Justification, JustificationKind,
        KnownIssue, Lifecycle, ManifestEntry, PatchStatus, Provenance, ReleaseHeader, RenderSpec,
        Risk, SCHEMA_VERSION, Sha256, UpstreamPrState, Visibility, blob_hash,
    };

    /// A small template that exercises the data paths the default asset uses:
    /// branding, the release header, `groupby("category")`, per-entry
    /// `public_summary`, and the optional `behavior_change`. Kept inline so this
    /// pure-crate test does not depend on the `crt` binary's asset.
    const TEST_TEMPLATE: &str = "\
# {{ branding.display_name }} — {{ release.name }}
{% for grouper, items in entries | groupby(\"category\") %}
## {{ grouper | title }}
{% for entry in items %}
- {{ entry.justification.public_summary }} ({{ entry.risk.component }})
{%- if entry.behavior_change %}
  behavior: {{ entry.behavior_change }}
{%- endif %}
{% endfor %}
{% endfor %}
{{ branding.footer }}";

    fn entry(category: &str, summary: &str, internal: Option<&str>) -> ManifestEntry {
        ManifestEntry {
            blob_hash: blob_hash(summary.as_bytes()),
            patch_id: "pid".to_owned(),
            order: 1,
            visibility: Visibility::Public,
            category: category.to_owned(),
            risk: Risk {
                component: "rgw".to_owned(),
                blast: Blast::Availability,
                conflict: Conflict::Trivial,
                coverage: Coverage::Partial,
            },
            justification: Justification {
                kind: JustificationKind::Engineering,
                refs: vec![],
                public_summary: summary.to_owned(),
                internal: internal.map(str::to_owned),
            },
            behavior_change: None,
            upgrade_notes: None,
            lifecycle: Lifecycle {
                status: PatchStatus::Active,
                first_shipped_in: None,
            },
            data_structure_change: None,
            provenance: Provenance::UpstreamPr {
                prs: vec![],
                commits: vec![],
                state: UpstreamPrState::MergedMain,
            },
        }
    }

    fn manifest(entries: Vec<ManifestEntry>) -> Manifest {
        Manifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseHeader {
                product: "ceph".to_owned(),
                namespace: "clyso-enterprise".to_owned(),
                channel: "ces".to_owned(),
                name: "ces-v18.2.0".to_owned(),
                base_ref: "v18.2.0".to_owned(),
                created: "2026-06-21T00:00:00+00:00".to_owned(),
                author: Identity {
                    name: "Releaser".to_owned(),
                    email: "rel@example.com".to_owned(),
                },
            },
            entries,
            known_issues: vec![KnownIssue {
                summary: "A known issue.".to_owned(),
                refs: vec![],
            }],
            upgrade_notes: None,
            branding: Branding {
                display_name: "Clyso Enterprise Storage".to_owned(),
                blurb: "blurb".to_owned(),
                footer: "the footer".to_owned(),
            },
            render: RenderSpec {
                minijinja_version: RENDER_MINIJINJA_VERSION.to_owned(),
                template_digest: blob_hash(b"template"),
            },
        }
    }

    #[test]
    fn renders_summaries_and_branding() {
        let m = manifest(vec![entry("fix", "Fixes a thing.", None)]);
        let out = render_notes(&m, TEST_TEMPLATE).unwrap();
        assert!(out.contains("Clyso Enterprise Storage — ces-v18.2.0"));
        assert!(out.contains("## Fix")); // `category` title-cased by the template
        assert!(out.contains("Fixes a thing. (rgw)"));
        assert!(out.contains("the footer"));
    }

    #[test]
    fn never_renders_the_internal_note() {
        // The default-style template renders the public summary and never the
        // internal note (design §7.2 / concept §6.5).
        let m = manifest(vec![entry(
            "fix",
            "Public summary.",
            Some("DO-NOT-LEAK internal note"),
        )]);
        let out = render_notes(&m, TEST_TEMPLATE).unwrap();
        assert!(out.contains("Public summary."));
        assert!(!out.contains("DO-NOT-LEAK"));
    }

    #[test]
    fn internal_is_redacted_from_the_template_context() {
        // Structural guarantee: even a template that *tries* to print the
        // internal note gets nothing, because it is stripped from the context
        // before rendering — not merely left unreferenced by the default
        // template.
        let m = manifest(vec![entry("fix", "Public.", Some("DO-NOT-LEAK secret"))]);
        let leaky = "summary=[{{ entries[0].justification.public_summary }}] \
                     internal=[{{ entries[0].justification.internal }}]";
        let out = render_notes(&m, leaky).unwrap();
        // The `entries[0].justification.…` accessor is live — the public summary
        // renders through it, so the internal note *would* surface here too were
        // it not stripped. It is, so neither the marker nor its words survive.
        assert!(out.contains("summary=[Public.]"));
        assert!(!out.contains("DO-NOT-LEAK"));
        assert!(!out.contains("secret"));
    }

    #[test]
    fn is_deterministic_across_renders() {
        // Two categories prove grouping order is stable, not insertion-dependent.
        let m = manifest(vec![
            entry("security", "A security fix.", None),
            entry("fix", "A bug fix.", None),
        ]);
        let a = render_notes(&m, TEST_TEMPLATE).unwrap();
        let b = render_notes(&m, TEST_TEMPLATE).unwrap();
        assert_eq!(a, b, "rendering the same manifest twice is byte-identical");
        // `groupby` orders groups by grouper: `fix` precedes `security`.
        let fix_at = a.find("## Fix").expect("fix group present");
        let sec_at = a.find("## Security").expect("security group present");
        assert!(fix_at < sec_at, "groups are ordered by category");
    }

    #[test]
    fn a_broken_template_is_an_error_not_a_panic() {
        let m = manifest(vec![entry("fix", "x", None)]);
        assert!(matches!(
            render_notes(&m, "{% for x in %}"),
            Err(CrtCoreError::Render(_))
        ));
    }

    #[test]
    fn version_gate_accepts_the_linked_version_and_rejects_others() {
        let ok = RenderSpec {
            minijinja_version: RENDER_MINIJINJA_VERSION.to_owned(),
            template_digest: Sha256::of(b"t"),
        };
        assert!(check_render_version(&ok).is_ok());

        let stale = RenderSpec {
            minijinja_version: "1.0.0".to_owned(),
            template_digest: Sha256::of(b"t"),
        };
        assert!(matches!(
            check_render_version(&stale),
            Err(CrtCoreError::RenderVersionMismatch { .. })
        ));
    }
}
