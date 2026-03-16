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

pub mod arch;
pub mod build;
pub mod ws;

pub use arch::Arch;
pub use build::{
    BuildComponent, BuildDescriptor, BuildDestImage, BuildId, BuildSignedOffBy, BuildState,
    BuildTarget, Priority, VersionType,
};

/// Worker token payload — the base64url-encoded JSON blob returned by the
/// server at worker registration. Contains everything the worker needs to
/// connect. Serialized by the server, deserialized by the worker.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkerToken {
    pub worker_id: String,
    pub worker_name: String,
    pub api_key: String,
    pub arch: String,
}
