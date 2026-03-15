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

/// Build architecture.
///
/// Canonical values are `x86_64` and `aarch64`. The alias `arm64` is accepted
/// on deserialization for compatibility with the existing Python `cbsdcore`
/// `BuildArch` enum which uses `arm64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Arch {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64", alias = "arm64")]
    Aarch64,
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X86_64 => write!(f, "x86_64"),
            Self::Aarch64 => write!(f, "aarch64"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_canonical() {
        assert_eq!(serde_json::to_string(&Arch::X86_64).unwrap(), r#""x86_64""#);
        assert_eq!(
            serde_json::to_string(&Arch::Aarch64).unwrap(),
            r#""aarch64""#
        );
    }

    #[test]
    fn deserialize_canonical() {
        assert_eq!(
            serde_json::from_str::<Arch>(r#""x86_64""#).unwrap(),
            Arch::X86_64
        );
        assert_eq!(
            serde_json::from_str::<Arch>(r#""aarch64""#).unwrap(),
            Arch::Aarch64
        );
    }

    #[test]
    fn deserialize_arm64_alias() {
        assert_eq!(
            serde_json::from_str::<Arch>(r#""arm64""#).unwrap(),
            Arch::Aarch64
        );
    }

    #[test]
    fn reject_unknown_arch() {
        assert!(serde_json::from_str::<Arch>(r#""riscv64""#).is_err());
    }
}
