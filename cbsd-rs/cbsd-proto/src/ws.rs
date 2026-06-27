// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

use serde::{Deserialize, Serialize};

use crate::build::{BuildDescriptor, BuildId, Priority};

// ---------------------------------------------------------------------------
// Server → Worker messages
// ---------------------------------------------------------------------------

/// Messages sent from server to worker over the WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Dispatch a build. Followed by a binary frame containing a single tar.gz
    /// that holds every component referenced by `descriptor.components`, each
    /// under its own `<name>/` top-level directory. The worker verifies
    /// `component_sha256` (computed over the combined archive) against the
    /// binary frame.
    BuildNew {
        build_id: BuildId,
        trace_id: String,
        priority: Priority,
        descriptor: Box<BuildDescriptor>,
        component_sha256: String,
    },

    /// Cancel a build (running or not yet accepted). If the worker receives
    /// this before sending `build_accepted`, it responds with
    /// `build_finished(revoked)` immediately.
    ///
    /// `reason` labels why the server sent the revoke (audit-rem D13). It is a
    /// server-side annotation only: the current worker ignores it and applies
    /// its normal revoke handling regardless (option A — see design 019 v2).
    /// The field is optional and `skip`-serialized when absent, so the wire
    /// shape is unchanged for the pre-D13 case and an older peer simply ignores
    /// it (SI-18: no `deny_unknown_fields`).
    BuildRevoke {
        build_id: BuildId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<BuildRevokeReason>,
    },

    /// Connection accepted. Sent after validating the worker's `hello`.
    Welcome {
        protocol_version: u32,
        connection_id: String,
        /// Worker validates its backoff ceiling against this value.
        grace_period_secs: u64,
        /// Whether the server wants this worker to push host/app metrics over
        /// the connection. A worker that does not understand the field (older
        /// build) defaults it to `false` and stays silent, so enabling metrics
        /// never breaks a rolling upgrade. `#[serde(default)]` makes an absent
        /// field decode to `false`; the server sets it from its metrics config.
        #[serde(default)]
        accepts_metrics: bool,
    },

    /// Connection or protocol error. Server closes the connection after this.
    Error {
        reason: String,
        min_version: Option<u32>,
        max_version: Option<u32>,
    },

    /// Server's reply to a worker lifecycle message that targeted a build the
    /// reporting connection does not own. Non-fatal: the worker MUST NOT
    /// close the connection in response. Per WCP D1/D2.
    UnauthorizedBuildAction {
        build_id: BuildId,
        action: WorkerBuildAction,
        reason: UnauthorizedBuildReason,
    },
}

/// Worker-to-server lifecycle action that can be rejected as unauthorized
/// when the reporting connection does not own the target build. Per WCP
/// D3, `WorkerStatus` is the first normative variant — it covers the case
/// where a reconnecting worker claims `Building` on a build it does not
/// actually own per the persisted `builds.worker_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerBuildAction {
    WorkerStatus,
    BuildAccepted,
    BuildStarted,
    BuildOutput,
    BuildFinished,
    BuildRejected,
}

/// Coarse reason exposed to workers for an unauthorized build action. Detail
/// stays in the server log; the wire enum is intentionally narrow per WCP D2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnauthorizedBuildReason {
    /// The build is not currently assigned to the reporting connection.
    NotAssigned,
}

/// Why the server sent a [`ServerMessage::BuildRevoke`] (audit-rem D13). This
/// is a server-side label for observability and protocol forward-compatibility;
/// the current worker ignores it (option A — see design 019 v2). It exists so a
/// future multi-connection worker could distinguish a migration supersede from
/// an admin revoke without a wire change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildRevokeReason {
    /// Operator/admin-initiated revoke (also the meaning of an absent reason).
    Admin,
    /// Sent (best-effort) on the OLD connection during a same-worker reconnect
    /// migration so a still-writable superseded connection stops work. The
    /// single-connection worker normally never reads this — the old socket is
    /// already dropped by the time it is sent (see design 019 v2).
    MigrationSupersede,
    /// Reporter-directed stray revoke from the WCP unauthorized-action path.
    UnauthorizedAction,
}

// ---------------------------------------------------------------------------
// Worker → Server messages
// ---------------------------------------------------------------------------

/// Messages sent from worker to server over the WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerMessage {
    /// First message after WebSocket connect (protocol v2). Auth is validated
    /// at HTTP upgrade, not in this message. The server derives worker identity
    /// from the API key used at upgrade — `worker_id` is no longer sent.
    Hello {
        protocol_version: u32,
        arch: crate::arch::Arch,
        cores_total: u32,
        ram_total_mb: u64,
        /// Worker binary version (e.g., "0.1.0+g3a7f2b1").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },

    /// Sent on reconnect ONLY if the worker is currently executing a build.
    /// Its absence after `hello` implies the worker is idle.
    WorkerStatus {
        state: WorkerReportedState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_id: Option<BuildId>,
    },

    /// Worker will run the build.
    BuildAccepted { build_id: BuildId },

    /// Worker cannot run the build (busy, incompatible, integrity failure).
    BuildRejected { build_id: BuildId, reason: String },

    /// Build execution has started (container launched).
    BuildStarted { build_id: BuildId },

    /// Build output. Batched: flushed every 200ms or 50 lines. Per-line seq:
    /// `start_seq`, `start_seq+1`, ..., `start_seq+len(lines)-1`.
    BuildOutput {
        build_id: BuildId,
        start_seq: u64,
        lines: Vec<String>,
    },

    /// Build completed (success, failure, or revoked).
    BuildFinished {
        build_id: BuildId,
        status: BuildFinishedStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_report: Option<serde_json::Value>,
    },

    /// Worker is shutting down gracefully (protocol v2). The server identifies
    /// the worker from the connection map — `worker_id` is no longer sent.
    WorkerStopping { reason: String },

    /// Periodic host + application metrics sample (design 021). Sent only when
    /// the server advertised `accepts_metrics` in its `Welcome`. The server
    /// re-exposes these under a `worker` label it stamps from the connection
    /// identity, so the worker never asserts its own label — bounding
    /// cardinality and preventing a worker from forging another's series.
    Metrics {
        /// Seconds since the worker process started — lets the server detect a
        /// worker restart (counter reset) without a separate signal.
        uptime_secs: u64,
        host: HostMetrics,
        app: AppMetrics,
    },
}

/// Host resource sample taken by the worker (design 021). All gauges are
/// point-in-time except the `*_total` fields, which are monotonic since-boot
/// counters the server exposes as Prometheus counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostMetrics {
    /// Fraction of CPU time spent non-idle since the previous sample, `0.0`–`1.0`.
    pub cpu_busy_ratio: f64,
    /// 1-minute load average.
    pub load1: f64,
    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
    pub mem_available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    /// Per-filesystem usage for the mounts the worker cares about (build spool,
    /// ccache). Bounded to a few entries to keep label cardinality small.
    pub filesystems: Vec<FilesystemUsage>,
    /// Monotonic since-boot disk byte counters across the host's block devices.
    pub disk_read_bytes_total: u64,
    pub disk_written_bytes_total: u64,
}

/// Usage of a single mounted filesystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilesystemUsage {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

/// Application-level sample sourced from the worker's own state (design 021):
/// ccache, build-subprocess outcomes, output-spool size, and dropped pushes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppMetrics {
    /// ccache statistics, absent when ccache is unavailable or not yet probed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccache: Option<CcacheMetrics>,
    pub subprocess_exits: SubprocessExitCounts,
    /// Current bytes held in the build output spool.
    pub spool_bytes: u64,
    /// Metrics samples the worker dropped rather than block the build path —
    /// nonzero means the server is missing samples and the push path is the
    /// bottleneck.
    pub push_drops_total: u64,
}

/// ccache size and effectiveness (from `ccache -s`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CcacheMetrics {
    pub size_bytes: u64,
    pub max_bytes: u64,
    /// Cache hit ratio `0.0`–`1.0` over ccache's own accounting window.
    pub hit_ratio: f64,
}

/// Build-subprocess exit tally since the worker started, partitioned by
/// outcome. `Copy` because it is a small fixed-size value passed by value
/// through the worker's collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubprocessExitCounts {
    pub success: u64,
    pub failure: u64,
    pub revoked: u64,
}

/// State reported by a worker on reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerReportedState {
    Idle,
    Building,
}

/// Build completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildFinishedStatus {
    Success,
    Failure,
    Revoked,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::Arch;
    use crate::build::{
        BuildComponent, BuildDestImage, BuildSignedOffBy, BuildTarget, VersionType,
    };
    use serde_json::{Value, json};
    use strum::IntoEnumIterator;

    #[test]
    fn server_message_build_new_round_trip() {
        let msg = ServerMessage::BuildNew {
            build_id: BuildId(42),
            trace_id: "abc-123".to_string(),
            priority: Priority::High,
            descriptor: Box::new(BuildDescriptor {
                version: "19.2.3".to_string(),
                channel: Some("ces".to_string()),
                version_type: Some(VersionType::Release),
                signed_off_by: BuildSignedOffBy {
                    user: "Alice".to_string(),
                    email: "alice@clyso.com".to_string(),
                },
                dst_image: BuildDestImage {
                    name: "harbor.clyso.com/ces/ceph".to_string(),
                    tag: "v19.2.3".to_string(),
                },
                components: vec![BuildComponent {
                    name: "ceph".to_string(),
                    git_ref: "v19.2.3".to_string(),
                    repo: None,
                }],
                build: BuildTarget {
                    distro: "rockylinux".to_string(),
                    os_version: "el9".to_string(),
                    artifact_type: "rpm".to_string(),
                    arch: Arch::X86_64,
                },
            }),
            component_sha256: "e3b0c44...".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"build_new""#));
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        if let ServerMessage::BuildNew { build_id, .. } = parsed {
            assert_eq!(build_id, BuildId(42));
        } else {
            panic!("expected BuildNew");
        }
    }

    #[test]
    fn server_message_welcome_includes_grace_period() {
        let msg = ServerMessage::Welcome {
            protocol_version: 1,
            connection_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            grace_period_secs: 90,
            accepts_metrics: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""grace_period_secs":90"#));
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        if let ServerMessage::Welcome {
            grace_period_secs, ..
        } = parsed
        {
            assert_eq!(grace_period_secs, 90);
        } else {
            panic!("expected Welcome");
        }
    }

    #[test]
    fn worker_message_hello_round_trip() {
        let msg = WorkerMessage::Hello {
            protocol_version: 2,
            arch: Arch::Aarch64,
            cores_total: 16,
            ram_total_mb: 65536,
            version: Some("0.1.0+gtest123".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"hello""#));
        assert!(json.contains(r#""arch":"aarch64""#));
        assert!(!json.contains("worker_id"));
        let parsed: WorkerMessage = serde_json::from_str(&json).unwrap();
        if let WorkerMessage::Hello { arch, version, .. } = parsed {
            assert_eq!(arch, Arch::Aarch64);
            assert_eq!(version.as_deref(), Some("0.1.0+gtest123"));
        } else {
            panic!("expected Hello");
        }
    }

    #[test]
    fn worker_message_hello_arm64_alias() {
        // No version field in JSON — tests backwards compat via serde(default).
        let json = r#"{"type":"hello","protocol_version":2,"arch":"arm64","cores_total":8,"ram_total_mb":32768}"#;
        let parsed: WorkerMessage = serde_json::from_str(json).unwrap();
        if let WorkerMessage::Hello { arch, version, .. } = parsed {
            assert_eq!(arch, Arch::Aarch64);
            assert_eq!(version, None);
        } else {
            panic!("expected Hello");
        }
    }

    #[test]
    fn worker_message_build_output_per_line_seq() {
        let msg = WorkerMessage::BuildOutput {
            build_id: BuildId(7),
            start_seq: 70,
            lines: vec!["line1".into(), "line2".into(), "line3".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""start_seq":70"#));
        let parsed: WorkerMessage = serde_json::from_str(&json).unwrap();
        if let WorkerMessage::BuildOutput {
            start_seq, lines, ..
        } = parsed
        {
            assert_eq!(start_seq, 70);
            assert_eq!(lines.len(), 3);
        } else {
            panic!("expected BuildOutput");
        }
    }

    #[test]
    fn worker_message_build_finished_with_error() {
        let msg = WorkerMessage::BuildFinished {
            build_id: BuildId(42),
            status: BuildFinishedStatus::Failure,
            error: Some("RPM build failed".to_string()),
            build_report: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""error":"RPM build failed""#));
        assert!(!json.contains("build_report"));
    }

    #[test]
    fn worker_message_build_finished_no_error() {
        let msg = WorkerMessage::BuildFinished {
            build_id: BuildId(42),
            status: BuildFinishedStatus::Success,
            error: None,
            build_report: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("error"));
        assert!(!json.contains("build_report"));
    }

    #[test]
    fn worker_message_build_finished_with_report() {
        let report = serde_json::json!({
            "report_version": 1,
            "version": "19.2.3",
            "skipped": false,
        });
        let msg = WorkerMessage::BuildFinished {
            build_id: BuildId(42),
            status: BuildFinishedStatus::Success,
            error: None,
            build_report: Some(report),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("build_report"));
        assert!(json.contains("report_version"));
        let parsed: WorkerMessage = serde_json::from_str(&json).unwrap();
        if let WorkerMessage::BuildFinished { build_report, .. } = parsed {
            assert!(build_report.is_some());
        } else {
            panic!("expected BuildFinished");
        }
    }

    #[test]
    fn worker_message_build_finished_missing_report_defaults_none() {
        // Older workers won't send the build_report field.
        let json = r#"{"type":"build_finished","build_id":42,"status":"success"}"#;
        let parsed: WorkerMessage = serde_json::from_str(json).unwrap();
        if let WorkerMessage::BuildFinished { build_report, .. } = parsed {
            assert!(build_report.is_none());
        } else {
            panic!("expected BuildFinished");
        }
    }

    #[test]
    fn worker_status_idle_no_build_id() {
        let msg = WorkerMessage::WorkerStatus {
            state: WorkerReportedState::Idle,
            build_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("build_id"));
    }

    #[test]
    fn worker_status_building_with_id() {
        let msg = WorkerMessage::WorkerStatus {
            state: WorkerReportedState::Building,
            build_id: Some(BuildId(42)),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""build_id":42"#));
    }

    #[test]
    fn welcome_missing_accepts_metrics_defaults_false() {
        // Older server → newer worker: the field is absent on the wire and must
        // decode to `false` so a worker never starts pushing unrequested.
        let json =
            r#"{"type":"welcome","protocol_version":2,"connection_id":"c","grace_period_secs":60}"#;
        let parsed: ServerMessage = serde_json::from_str(json).unwrap();
        match parsed {
            ServerMessage::Welcome {
                accepts_metrics, ..
            } => assert!(!accepts_metrics, "absent accepts_metrics must be false"),
            other => panic!("expected Welcome, got {other:?}"),
        }
    }

    #[test]
    fn worker_message_metrics_round_trip() {
        let msg = WorkerMessage::Metrics {
            uptime_secs: 3600,
            host: HostMetrics {
                cpu_busy_ratio: 0.42,
                load1: 1.5,
                mem_total_bytes: 64 * 1024 * 1024 * 1024,
                mem_used_bytes: 12 * 1024 * 1024 * 1024,
                mem_available_bytes: 52 * 1024 * 1024 * 1024,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
                filesystems: vec![FilesystemUsage {
                    mount: "/cbs/spool".to_string(),
                    total_bytes: 500 * 1024 * 1024 * 1024,
                    used_bytes: 120 * 1024 * 1024 * 1024,
                }],
                disk_read_bytes_total: 1_000_000,
                disk_written_bytes_total: 2_000_000,
            },
            app: AppMetrics {
                ccache: Some(CcacheMetrics {
                    size_bytes: 8 * 1024 * 1024 * 1024,
                    max_bytes: 20 * 1024 * 1024 * 1024,
                    hit_ratio: 0.87,
                }),
                subprocess_exits: SubprocessExitCounts {
                    success: 10,
                    failure: 2,
                    revoked: 1,
                },
                spool_bytes: 42,
                push_drops_total: 0,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"metrics""#));
        assert!(json.contains(r#""cpu_busy_ratio":0.42"#));
        assert!(json.contains(r#""mount":"/cbs/spool""#));
        let parsed: WorkerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            WorkerMessage::Metrics {
                uptime_secs,
                host,
                app,
            } => {
                assert_eq!(uptime_secs, 3600);
                assert_eq!(host.filesystems.len(), 1);
                assert_eq!(app.subprocess_exits.failure, 2);
                assert_eq!(app.ccache.unwrap().hit_ratio, 0.87);
            }
            other => panic!("expected Metrics, got {other:?}"),
        }
    }

    #[test]
    fn app_metrics_absent_ccache_omits_field() {
        // ccache may be unavailable; absent ccache must skip-serialize so the
        // wire shape stays minimal and an older reader sees no surprise field.
        let app = AppMetrics {
            ccache: None,
            subprocess_exits: SubprocessExitCounts {
                success: 0,
                failure: 0,
                revoked: 0,
            },
            spool_bytes: 0,
            push_drops_total: 0,
        };
        let json = serde_json::to_string(&app).unwrap();
        assert!(
            !json.contains("ccache"),
            "None ccache must be omitted: {json}"
        );
    }

    #[test]
    fn ccache_metrics_tolerates_unknown_field() {
        // SI-18: `CcacheMetrics` must not gain `deny_unknown_fields` — a newer
        // worker adding a stat here must still decode on an older server. The
        // metrics-variant SI-18 case omits ccache (it is `None`), so this is the
        // only coverage for that struct's forward compatibility.
        let json = r#"{"size_bytes":1,"max_bytes":2,"hit_ratio":0.5,"future_field":"x"}"#;
        let parsed: CcacheMetrics = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.size_bytes, 1);
        assert_eq!(parsed.hit_ratio, 0.5);
    }

    #[test]
    fn worker_build_action_worker_status_serdes_as_snake_case() {
        let msg = ServerMessage::UnauthorizedBuildAction {
            build_id: BuildId(11),
            action: WorkerBuildAction::WorkerStatus,
            reason: UnauthorizedBuildReason::NotAssigned,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""action":"worker_status""#));
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::UnauthorizedBuildAction { action, .. } => {
                assert_eq!(action, WorkerBuildAction::WorkerStatus);
            }
            _ => panic!("expected UnauthorizedBuildAction"),
        }
    }

    #[test]
    fn server_message_unauthorized_build_action_round_trip() {
        let msg = ServerMessage::UnauthorizedBuildAction {
            build_id: BuildId(7),
            action: WorkerBuildAction::BuildStarted,
            reason: UnauthorizedBuildReason::NotAssigned,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"unauthorized_build_action""#));
        assert!(json.contains(r#""action":"build_started""#));
        assert!(json.contains(r#""reason":"not_assigned""#));
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::UnauthorizedBuildAction {
                build_id,
                action,
                reason,
            } => {
                assert_eq!(build_id, BuildId(7));
                assert_eq!(action, WorkerBuildAction::BuildStarted);
                assert_eq!(reason, UnauthorizedBuildReason::NotAssigned);
            }
            _ => panic!("expected UnauthorizedBuildAction"),
        }
    }

    #[test]
    fn server_message_error_with_version_range() {
        let msg = ServerMessage::Error {
            reason: "unsupported protocol version 3; server supports 1".to_string(),
            min_version: Some(1),
            max_version: Some(1),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""min_version":1"#));
    }

    // audit-rem D13: BuildRevoke.reason wire compatibility (both directions).

    #[test]
    fn build_revoke_absent_reason_deserializes_to_none() {
        // Old server → new worker: no `reason` field on the wire.
        let parsed: ServerMessage =
            serde_json::from_str(r#"{"type":"build_revoke","build_id":7}"#).unwrap();
        match parsed {
            ServerMessage::BuildRevoke { build_id, reason } => {
                assert_eq!(build_id, BuildId(7));
                assert_eq!(reason, None, "absent reason must deserialize to None");
            }
            other => panic!("expected BuildRevoke, got {other:?}"),
        }
    }

    #[test]
    fn build_revoke_none_reason_omits_field_on_wire() {
        // New server, uncategorized revoke → wire shape identical to pre-D13.
        let msg = ServerMessage::BuildRevoke {
            build_id: BuildId(7),
            reason: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("reason"),
            "None reason must be skip-serialized for wire compatibility: {json}"
        );
    }

    #[test]
    fn build_revoke_some_reason_round_trips() {
        let msg = ServerMessage::BuildRevoke {
            build_id: BuildId(7),
            reason: Some(BuildRevokeReason::MigrationSupersede),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""reason":"migration_supersede""#));
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            ServerMessage::BuildRevoke {
                reason: Some(BuildRevokeReason::MigrationSupersede),
                ..
            }
        ));
    }

    #[test]
    fn build_revoke_unknown_reason_value_is_rejected() {
        // An unknown *value* for the known `reason` field is rejected by serde
        // regardless of deny_unknown_fields (which SI-18 forbids on the type).
        let result: Result<ServerMessage, _> =
            serde_json::from_str(r#"{"type":"build_revoke","build_id":7,"reason":"bogus"}"#);
        assert!(
            result.is_err(),
            "unknown BuildRevokeReason value must be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // SI-18 / D13-T6: `ServerMessage` must accept unknown fields, or a newer
    // peer's added field breaks deserialization on an older peer mid-rolling-
    // upgrade. serde rejects `#[serde(deny_unknown_fields)]` placed directly on
    // an enum variant at COMPILE time, so this runtime test guards the forms
    // that would otherwise slip through: the attribute on the `ServerMessage`
    // enum container, or on a standalone struct that a variant's payload is
    // refactored into. It also drags any newly added variant through
    // compile-time gates (witness, tag, sentinel) before it can land without a
    // deserialization case. See design 019 D13-T6.
    // -----------------------------------------------------------------------

    /// Build a valid `BuildDescriptor` for SI-18 test payloads. Mirrors
    /// `server_message_build_new_round_trip`'s explicit construction because
    /// `BuildDescriptor` and its nested types do not impl `Default`.
    fn test_descriptor() -> BuildDescriptor {
        BuildDescriptor {
            version: "test".to_string(),
            channel: None,
            version_type: None,
            signed_off_by: BuildSignedOffBy {
                user: "test".to_string(),
                email: "test@example.com".to_string(),
            },
            dst_image: BuildDestImage {
                name: "test-image".to_string(),
                tag: "test-tag".to_string(),
            },
            components: vec![BuildComponent {
                name: "test-component".to_string(),
                git_ref: "v0".to_string(),
                repo: None,
            }],
            build: BuildTarget {
                distro: "rockylinux".to_string(),
                os_version: "el9".to_string(),
                artifact_type: "rpm".to_string(),
                arch: Arch::X86_64,
            },
        }
    }

    /// JSON form of `test_descriptor()`. Always succeeds because
    /// `BuildDescriptor` derives `Serialize`.
    fn test_descriptor_json() -> Value {
        serde_json::to_value(test_descriptor()).unwrap()
    }

    /// Test-only companion enum mirroring `ServerMessage`'s variants without
    /// their associated data. `strum::EnumIter` derives `iter()`, which yields
    /// every variant — the runtime-enumeration mechanism that closes the
    /// "witness updated, case forgotten" gap.
    ///
    /// `Hash` is intentionally NOT derived: `cases()` is keyed by the wire
    /// string, not by this enum, so a `Hash` impl would be unused.
    #[derive(strum::EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
    enum ServerMessageTag {
        BuildNew,
        BuildRevoke,
        Welcome,
        Error,
        UnauthorizedBuildAction,
    }

    impl ServerMessageTag {
        /// Compile-time witness. Exhaustive on `ServerMessage` — `rustc`
        /// rejects this with a non-exhaustive-match error if a new variant is
        /// added to `ServerMessage` without a corresponding arm. Each arm maps
        /// to a `ServerMessageTag` variant, so a missing tag-enum variant
        /// surfaces as an "unknown variant" compile error here too.
        fn from_message(msg: &ServerMessage) -> Self {
            match msg {
                ServerMessage::BuildNew { .. } => Self::BuildNew,
                ServerMessage::BuildRevoke { .. } => Self::BuildRevoke,
                ServerMessage::Welcome { .. } => Self::Welcome,
                ServerMessage::Error { .. } => Self::Error,
                ServerMessage::UnauthorizedBuildAction { .. } => Self::UnauthorizedBuildAction,
            }
        }

        /// Wire-format tag (the serde `"type"` discriminator). Exhaustive on
        /// `Self` — `rustc` forces an arm when a variant is added to
        /// `ServerMessageTag`.
        fn as_wire(&self) -> &'static str {
            match self {
                Self::BuildNew => "build_new",
                Self::BuildRevoke => "build_revoke",
                Self::Welcome => "welcome",
                Self::Error => "error",
                Self::UnauthorizedBuildAction => "unauthorized_build_action",
            }
        }
    }

    /// Construct a sentinel `ServerMessage` for a given tag. Exhaustive on the
    /// tag enum, so a new tag variant is compile-forced to add an arm. Every
    /// field is explicit because the underlying types do not impl `Default`.
    fn sentinel_for_tag(tag: ServerMessageTag) -> ServerMessage {
        match tag {
            ServerMessageTag::BuildNew => ServerMessage::BuildNew {
                build_id: BuildId(0),
                trace_id: String::new(),
                priority: Priority::default(),
                descriptor: Box::new(test_descriptor()),
                component_sha256: String::new(),
            },
            ServerMessageTag::BuildRevoke => ServerMessage::BuildRevoke {
                build_id: BuildId(0),
                reason: None,
            },
            ServerMessageTag::Welcome => ServerMessage::Welcome {
                protocol_version: 2,
                connection_id: String::new(),
                grace_period_secs: 0,
                accepts_metrics: false,
            },
            ServerMessageTag::Error => ServerMessage::Error {
                reason: String::new(),
                min_version: None,
                max_version: None,
            },
            ServerMessageTag::UnauthorizedBuildAction => ServerMessage::UnauthorizedBuildAction {
                build_id: BuildId(0),
                action: WorkerBuildAction::WorkerStatus,
                reason: UnauthorizedBuildReason::NotAssigned,
            },
        }
    }

    /// JSON payloads for the SI-18 deserialization check, keyed by wire-format
    /// tag. Each payload matches its variant's field schema plus an injected
    /// `future_field` to exercise the unknown-field path.
    ///
    /// `cases()` is the only coordinated list NOT compile-forced; a missing
    /// entry trips the runtime assertion in
    /// `no_deny_unknown_fields_on_server_message`.
    fn cases() -> Vec<(&'static str, Value)> {
        vec![
            (
                "build_new",
                json!({
                    "type": "build_new",
                    "build_id": 42,
                    "trace_id": "00000000-0000-0000-0000-000000000000",
                    "priority": "normal",
                    "descriptor": test_descriptor_json(),
                    "component_sha256": "0".repeat(64),
                    "future_field": "x",
                }),
            ),
            (
                "build_revoke",
                json!({
                    "type": "build_revoke",
                    "build_id": 42,
                    "future_field": "x",
                }),
            ),
            (
                "welcome",
                json!({
                    "type": "welcome",
                    "protocol_version": 2,
                    "connection_id": "test-conn-id",
                    "grace_period_secs": 60,
                    "future_field": "x",
                }),
            ),
            (
                "error",
                json!({
                    "type": "error",
                    "reason": "test",
                    "min_version": null,
                    "max_version": null,
                    "future_field": "x",
                }),
            ),
            (
                "unauthorized_build_action",
                json!({
                    "type": "unauthorized_build_action",
                    "build_id": 42,
                    "action": "worker_status",
                    "reason": "not_assigned",
                    "future_field": "x",
                }),
            ),
        ]
    }

    #[test]
    fn no_deny_unknown_fields_on_server_message() {
        let cases_map: std::collections::HashMap<&'static str, Value> =
            cases().into_iter().collect();

        // Runtime exhaustiveness over ALL ServerMessageTag variants.
        // `iter()` (strum) auto-extends when a variant is added. The sentinel
        // match is compile-forced; verify the witness round-trips the sentinel
        // and that a case exists for the tag.
        for tag in ServerMessageTag::iter() {
            let wire = tag.as_wire();
            let sentinel = sentinel_for_tag(tag);
            let witnessed = ServerMessageTag::from_message(&sentinel);
            assert_eq!(
                tag, witnessed,
                "sentinel/witness drift for tag `{wire}`: from_message returned \
                 a different ServerMessageTag",
            );
            assert!(
                cases_map.contains_key(wire),
                "ServerMessageTag::{tag:?} (wire `{wire}`) has no entry in \
                 cases() — SI-18 is not enforced for this variant. Add a case \
                 to cases() in cbsd-proto/src/ws.rs.",
            );
        }

        // Per-variant deserialization: each payload carries an unknown
        // `future_field`; deserialization must succeed. Fails if
        // `deny_unknown_fields` is added to the `ServerMessage` enum (or to a
        // standalone struct a variant's payload is refactored into).
        for (wire, payload) in cases_map {
            let result: Result<ServerMessage, _> = serde_json::from_value(payload);
            assert!(
                result.is_ok(),
                "ServerMessage `{wire}` rejected an unknown field — likely \
                 `#[serde(deny_unknown_fields)]` was added to the ServerMessage \
                 enum or to this variant's payload struct; that violates SI-18 \
                 and breaks rolling upgrades. See design 019 D13-T6. Error: {:?}",
                result.err(),
            );
            // Confirm the deserialized variant matches its expected wire tag.
            let msg = result.unwrap();
            let witnessed = ServerMessageTag::from_message(&msg).as_wire();
            assert_eq!(
                wire, witnessed,
                "case payload tagged `{wire}` deserialized to wire tag \
                 `{witnessed}` — case-tag/payload-type drift",
            );
        }
    }

    // -----------------------------------------------------------------------
    // SI-18 parity for `WorkerMessage`. The worker→server direction needs the
    // same forward-compatibility guarantee: a newer worker that adds a field
    // (e.g. to the metrics payload) must not break an older server. This
    // mirrors the `ServerMessage` machinery so a new `WorkerMessage` variant is
    // dragged through the same compile-time gates (witness, tag, sentinel)
    // before it can land without a deserialization case.
    // -----------------------------------------------------------------------

    fn test_host_metrics() -> HostMetrics {
        HostMetrics {
            cpu_busy_ratio: 0.0,
            load1: 0.0,
            mem_total_bytes: 0,
            mem_used_bytes: 0,
            mem_available_bytes: 0,
            swap_total_bytes: 0,
            swap_used_bytes: 0,
            filesystems: Vec::new(),
            disk_read_bytes_total: 0,
            disk_written_bytes_total: 0,
        }
    }

    fn test_app_metrics() -> AppMetrics {
        AppMetrics {
            ccache: None,
            subprocess_exits: SubprocessExitCounts {
                success: 0,
                failure: 0,
                revoked: 0,
            },
            spool_bytes: 0,
            push_drops_total: 0,
        }
    }

    /// Companion enum mirroring `WorkerMessage`'s variants without their data.
    #[derive(strum::EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
    enum WorkerMessageTag {
        Hello,
        WorkerStatus,
        BuildAccepted,
        BuildRejected,
        BuildStarted,
        BuildOutput,
        BuildFinished,
        WorkerStopping,
        Metrics,
    }

    impl WorkerMessageTag {
        /// Compile-time witness; exhaustive on `WorkerMessage`.
        fn from_message(msg: &WorkerMessage) -> Self {
            match msg {
                WorkerMessage::Hello { .. } => Self::Hello,
                WorkerMessage::WorkerStatus { .. } => Self::WorkerStatus,
                WorkerMessage::BuildAccepted { .. } => Self::BuildAccepted,
                WorkerMessage::BuildRejected { .. } => Self::BuildRejected,
                WorkerMessage::BuildStarted { .. } => Self::BuildStarted,
                WorkerMessage::BuildOutput { .. } => Self::BuildOutput,
                WorkerMessage::BuildFinished { .. } => Self::BuildFinished,
                WorkerMessage::WorkerStopping { .. } => Self::WorkerStopping,
                WorkerMessage::Metrics { .. } => Self::Metrics,
            }
        }

        /// Wire-format tag (the serde `"type"` discriminator).
        fn as_wire(&self) -> &'static str {
            match self {
                Self::Hello => "hello",
                Self::WorkerStatus => "worker_status",
                Self::BuildAccepted => "build_accepted",
                Self::BuildRejected => "build_rejected",
                Self::BuildStarted => "build_started",
                Self::BuildOutput => "build_output",
                Self::BuildFinished => "build_finished",
                Self::WorkerStopping => "worker_stopping",
                Self::Metrics => "metrics",
            }
        }
    }

    /// Construct a sentinel `WorkerMessage` for a given tag; exhaustive on the
    /// tag enum, so a new tag variant is compile-forced to add an arm.
    fn sentinel_for_worker_tag(tag: WorkerMessageTag) -> WorkerMessage {
        match tag {
            WorkerMessageTag::Hello => WorkerMessage::Hello {
                protocol_version: 2,
                arch: Arch::X86_64,
                cores_total: 0,
                ram_total_mb: 0,
                version: None,
            },
            WorkerMessageTag::WorkerStatus => WorkerMessage::WorkerStatus {
                state: WorkerReportedState::Idle,
                build_id: None,
            },
            WorkerMessageTag::BuildAccepted => WorkerMessage::BuildAccepted {
                build_id: BuildId(0),
            },
            WorkerMessageTag::BuildRejected => WorkerMessage::BuildRejected {
                build_id: BuildId(0),
                reason: String::new(),
            },
            WorkerMessageTag::BuildStarted => WorkerMessage::BuildStarted {
                build_id: BuildId(0),
            },
            WorkerMessageTag::BuildOutput => WorkerMessage::BuildOutput {
                build_id: BuildId(0),
                start_seq: 0,
                lines: Vec::new(),
            },
            WorkerMessageTag::BuildFinished => WorkerMessage::BuildFinished {
                build_id: BuildId(0),
                status: BuildFinishedStatus::Success,
                error: None,
                build_report: None,
            },
            WorkerMessageTag::WorkerStopping => WorkerMessage::WorkerStopping {
                reason: String::new(),
            },
            WorkerMessageTag::Metrics => WorkerMessage::Metrics {
                uptime_secs: 0,
                host: test_host_metrics(),
                app: test_app_metrics(),
            },
        }
    }

    /// JSON payloads for the SI-18 check, keyed by wire tag; each carries an
    /// injected `future_field` to exercise the unknown-field path.
    fn worker_cases() -> Vec<(&'static str, Value)> {
        vec![
            (
                "hello",
                json!({
                    "type": "hello",
                    "protocol_version": 2,
                    "arch": "x86_64",
                    "cores_total": 8,
                    "ram_total_mb": 32768,
                    "future_field": "x",
                }),
            ),
            (
                "worker_status",
                json!({
                    "type": "worker_status",
                    "state": "idle",
                    "future_field": "x",
                }),
            ),
            (
                "build_accepted",
                json!({"type": "build_accepted", "build_id": 1, "future_field": "x"}),
            ),
            (
                "build_rejected",
                json!({"type": "build_rejected", "build_id": 1, "reason": "no", "future_field": "x"}),
            ),
            (
                "build_started",
                json!({"type": "build_started", "build_id": 1, "future_field": "x"}),
            ),
            (
                "build_output",
                json!({
                    "type": "build_output",
                    "build_id": 1,
                    "start_seq": 0,
                    "lines": [],
                    "future_field": "x",
                }),
            ),
            (
                "build_finished",
                json!({
                    "type": "build_finished",
                    "build_id": 1,
                    "status": "success",
                    "future_field": "x",
                }),
            ),
            (
                "worker_stopping",
                json!({"type": "worker_stopping", "reason": "bye", "future_field": "x"}),
            ),
            (
                "metrics",
                json!({
                    "type": "metrics",
                    "uptime_secs": 1,
                    "host": {
                        "cpu_busy_ratio": 0.0,
                        "load1": 0.0,
                        "mem_total_bytes": 0,
                        "mem_used_bytes": 0,
                        "mem_available_bytes": 0,
                        "swap_total_bytes": 0,
                        "swap_used_bytes": 0,
                        "filesystems": [],
                        "disk_read_bytes_total": 0,
                        "disk_written_bytes_total": 0,
                        "future_field": "x",
                    },
                    "app": {
                        "subprocess_exits": {
                            "success": 0,
                            "failure": 0,
                            "revoked": 0,
                            "future_field": "x",
                        },
                        "spool_bytes": 0,
                        "push_drops_total": 0,
                        "future_field": "x",
                    },
                    "future_field": "x",
                }),
            ),
        ]
    }

    #[test]
    fn no_deny_unknown_fields_on_worker_message() {
        let cases_map: std::collections::HashMap<&'static str, Value> =
            worker_cases().into_iter().collect();

        for tag in WorkerMessageTag::iter() {
            let wire = tag.as_wire();
            let sentinel = sentinel_for_worker_tag(tag);
            let witnessed = WorkerMessageTag::from_message(&sentinel);
            assert_eq!(
                tag, witnessed,
                "sentinel/witness drift for tag `{wire}`: from_message returned \
                 a different WorkerMessageTag",
            );
            assert!(
                cases_map.contains_key(wire),
                "WorkerMessageTag::{tag:?} (wire `{wire}`) has no entry in \
                 worker_cases() — SI-18 is not enforced for this variant. Add a \
                 case to worker_cases() in cbsd-proto/src/ws.rs.",
            );
        }

        for (wire, payload) in cases_map {
            let result: Result<WorkerMessage, _> = serde_json::from_value(payload);
            assert!(
                result.is_ok(),
                "WorkerMessage `{wire}` rejected an unknown field — likely \
                 `#[serde(deny_unknown_fields)]` was added to the WorkerMessage \
                 enum or to a nested payload struct; that violates SI-18 and \
                 breaks rolling upgrades. Error: {:?}",
                result.err(),
            );
            let msg = result.unwrap();
            let witnessed = WorkerMessageTag::from_message(&msg).as_wire();
            assert_eq!(
                wire, witnessed,
                "case payload tagged `{wire}` deserialized to wire tag \
                 `{witnessed}` — case-tag/payload-type drift",
            );
        }
    }
}
