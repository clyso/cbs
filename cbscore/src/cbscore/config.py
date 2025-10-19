# CBS Core - config
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.

from __future__ import annotations

from pathlib import Path

import pydantic

from cbscore.errors import CESError


class ConfigError(CESError):
    pass


class VaultUserPassConfig(pydantic.BaseModel):
    username: str
    password: str


class VaultAppRoleConfig(pydantic.BaseModel):
    role_id: str
    secret_id: str


class VaultConfig(pydantic.BaseModel):
    vault_addr: str
    vault_transit: str
    auth_user: VaultUserPassConfig | None
    auth_approle: VaultAppRoleConfig | None
    auth_token: str | None

    @classmethod
    def load(cls, path: Path) -> VaultConfig:
        if not path.exists() or not path.is_file():
            raise ConfigError(
                f"vault config file {path} does not exist or is not a file"
            )

        try:
            return VaultConfig.model_validate_json(path.read_text())
        except Exception as e:
            raise ConfigError(f"failed to load vault config from {path}: {e}") from e


class Config(pydantic.BaseModel):
    components_path: list[Path]
    secrets_path: Path
    scratch_path: Path
    scratch_containers_path: Path
    ccache_path: Path | None = None

    @classmethod
    def load(cls, path: Path) -> Config:
        if not path.exists() or not path.is_file():
            raise ConfigError(f"config file {path} does not exist or is not a file")

        try:
            return Config.model_validate_json(path.read_text())
        except Exception as e:
            raise ConfigError(f"failed to load config from {path}: {e}") from e
