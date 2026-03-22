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
    /// Dispatch a build. Followed by a binary frame containing the component
    /// tar.gz. The worker verifies `component_sha256` against the binary frame.
    BuildNew {
        build_id: BuildId,
        trace_id: String,
        priority: Priority,
        descriptor: BuildDescriptor,
        component_sha256: String,
    },

    /// Cancel a build (running or not yet accepted). If the worker receives
    /// this before sending `build_accepted`, it responds with
    /// `build_finished(revoked)` immediately.
    BuildRevoke { build_id: BuildId },

    /// Connection accepted. Sent after validating the worker's `hello`.
    Welcome {
        protocol_version: u32,
        connection_id: String,
        /// Worker validates its backoff ceiling against this value.
        grace_period_secs: u64,
    },

    /// Connection or protocol error. Server closes the connection after this.
    Error {
        reason: String,
        min_version: Option<u32>,
        max_version: Option<u32>,
    },
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

    #[test]
    fn server_message_build_new_round_trip() {
        let msg = ServerMessage::BuildNew {
            build_id: BuildId(42),
            trace_id: "abc-123".to_string(),
            priority: Priority::High,
            descriptor: BuildDescriptor {
                version: "19.2.3".to_string(),
                channel: "ces".to_string(),
                version_type: VersionType::Release,
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
            },
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
    fn server_message_error_with_version_range() {
        let msg = ServerMessage::Error {
            reason: "unsupported protocol version 3; server supports 1".to_string(),
            min_version: Some(1),
            max_version: Some(1),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""min_version":1"#));
    }
}
