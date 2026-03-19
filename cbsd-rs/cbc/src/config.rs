// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Error;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub host: String,
    pub token: String,
}

impl Config {
    /// Load configuration from the given path, or from the default location.
    ///
    /// Resolution order:
    /// 1. Explicit `path` argument.
    /// 2. `dirs::config_dir()/cbc/config.json`.
    pub fn load(path: Option<&Path>) -> Result<Self, Error> {
        let p = match path {
            Some(p) => p.to_path_buf(),
            None => Self::default_path()
                .ok_or_else(|| Error::Config("cannot determine config directory".into()))?,
        };

        let contents = std::fs::read_to_string(&p)
            .map_err(|e| Error::Config(format!("cannot read {}: {e}", p.display())))?;

        serde_json::from_str(&contents)
            .map_err(|e| Error::Config(format!("invalid config at {}: {e}", p.display())))
    }

    /// Persist this configuration to disk as JSON.
    ///
    /// Creates parent directories if needed and restricts file permissions to
    /// 0600 on Unix.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Config(format!("cannot create directory {}: {e}", parent.display()))
            })?;
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("cannot serialize config: {e}")))?;

        std::fs::write(path, &json)
            .map_err(|e| Error::Config(format!("cannot write {}: {e}", path.display())))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
                |e| Error::Config(format!("cannot set permissions on {}: {e}", path.display())),
            )?;
        }

        Ok(())
    }

    /// Return the default config file path: `$XDG_CONFIG_HOME/cbc/config.json`
    /// (or platform equivalent via `dirs::config_dir`).
    ///
    /// Returns `None` when the platform config directory cannot be determined.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("cbc").join("config.json"))
    }
}
