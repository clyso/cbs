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

import json
from pathlib import Path
from typing import Annotated, ClassVar

import pydantic
import yaml

from cbscore.errors import CESError
from cbscore.logger import logger as root_logger
from cbscore.utils.secrets import SecretsError
from cbscore.utils.secrets.models import Secrets

logger = root_logger.getChild("config")


class ConfigError(CESError):
    pass


class VaultUserPassConfig(pydantic.BaseModel):
    username: str
    password: str


class VaultAppRoleConfig(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    role_id: Annotated[str, pydantic.Field(alias="role-id")]
    secret_id: Annotated[str, pydantic.Field(alias="secred-id")]


class VaultConfig(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    vault_addr: Annotated[str, pydantic.Field(alias="vault-addr")]
    auth_user: Annotated[
        VaultUserPassConfig | None, pydantic.Field(alias="auth-user", default=None)
    ]
    auth_approle: Annotated[
        VaultAppRoleConfig | None, pydantic.Field(alias="auth-approle", default=None)
    ]
    auth_token: Annotated[str | None, pydantic.Field(alias="auth-token", default=None)]

    @classmethod
    def load(cls, path: Path) -> VaultConfig:
        if not path.exists() or not path.is_file():
            raise ConfigError(
                f"vault config file '{path}' does not exist or is not a file"
            )

        try:
            raw_data = path.read_text()
            if path.suffix.lower() == ".yaml":
                config = VaultConfig.model_validate(yaml.safe_load(raw_data))
            else:
                config = VaultConfig.model_validate_json(raw_data)

        except (yaml.YAMLError, pydantic.ValidationError) as e:
            msg = f"error loading vault config at '{path}': {e}"
            logger.error(msg)
            raise ConfigError(msg) from e
        except Exception as e:
            msg = f"unexpected error loading vault config at '{path}': {e}"
            logger.error(msg)
            raise ConfigError(msg) from e

        return config

    def store(self, path: Path) -> None:
        """Store vault config to specified path in YAML format."""
        try:
            raw_data = yaml.safe_dump(self.model_dump(), indent=2)
            _ = path.write_text(raw_data)
        except Exception as e:
            msg = f"error storing vault config to '{path}': {e}"
            logger.error(msg)
            raise ConfigError(msg) from e


class PathsConfig(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_name=True,
        validate_by_alias=True,
        serialize_by_alias=True,
    )

    components: list[Path]
    scratch: Path
    scratch_containers: Annotated[Path, pydantic.Field(alias="scratch-containers")]
    ccache: Path | None = None


class ArtifactsS3Config(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    s3_artifact_bucket: Annotated[str, pydantic.Field(alias="s3-artifact-bucket")]
    s3_releases_bucket: Annotated[str, pydantic.Field(alias="s3-releases-bucket")]


class ArtifactsConfig(pydantic.BaseModel):
    s3: ArtifactsS3Config | None = pydantic.Field(default=None)


class DefaultSecretsConfig(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        populate_by_name=True,
        validate_by_alias=True,
        serialize_by_alias=True,
    )

    storage: str | None = None
    gpg_signing: Annotated[
        str | None, pydantic.Field(alias="gpg-signing", default=None)
    ] = None
    transit_signing: Annotated[
        str | None, pydantic.Field(alias="transit-signing", default=None)
    ] = None
    registry: str | None = None


class Config(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        populate_by_name=True,
        validate_by_alias=True,
        serialize_by_alias=True,
    )

    paths: PathsConfig
    artifacts: ArtifactsConfig | None = pydantic.Field(default=None)
    secrets_config: Annotated[
        DefaultSecretsConfig | None,
        pydantic.Field(default=None, alias="secrets-config"),
    ]
    secrets: list[Path] = pydantic.Field(default=[])
    vault: Path | None = pydantic.Field(default=None)

    @classmethod
    def load(cls, path: Path) -> Config:
        if not path.exists() or not path.is_file():
            raise ConfigError(f"config file '{path}' does not exist or is not a file")

        try:
            raw_data = path.read_text()
            if path.suffix.lower() == ".yaml":
                config = Config.model_validate(yaml.safe_load(raw_data))
            else:
                config = Config.model_validate_json(raw_data)

        except (yaml.YAMLError, pydantic.ValidationError) as e:
            msg = f"error loading config at '{path}': {e}"
            logger.error(msg)
            raise ConfigError(msg) from e
        except Exception as e:
            msg = f"unexpected error loading config at '{path}': {e}"
            logger.error(msg)
            raise ConfigError(msg) from e

        return config

    def store(self, path: Path) -> None:
        """Store config to specified path in YAML format."""
        try:
            # we need to do this because the model contains non-serializable Path
            # objects, and we need these to be handled by pydantic's JSON
            # serializer first.
            json_dict = json.loads(self.model_dump_json())  # pyright: ignore[reportAny]
            raw_data = yaml.safe_dump(json_dict, indent=2)
            _ = path.write_text(raw_data)
        except Exception as e:
            msg = f"error storing config to '{path}': {e}"
            logger.error(msg)
            raise ConfigError(msg) from e

    def get_secrets(self) -> Secrets:
        """Obtain merged secrets from all configured secrets files."""
        secrets: Secrets | None = None
        for secrets_path in self.secrets:
            logger.debug(f"loading secrets from '{secrets_path}'")
            try:
                loaded_secrets = Secrets.load(secrets_path)
            except SecretsError as e:
                msg = f"error loading secrets from '{secrets_path}': {e}"
                logger.error(msg)
                raise ConfigError(msg) from e

            if not secrets:
                secrets = loaded_secrets
            else:
                secrets.merge(loaded_secrets)

        if not secrets:
            msg = "no secrets defined in config"
            logger.error(msg)
            raise ConfigError(msg)

        return secrets

    def get_vault_config(self) -> VaultConfig | None:
        """Obtain vault configuration, if any."""
        if not self.vault:
            return None
        return VaultConfig.load(self.vault)
