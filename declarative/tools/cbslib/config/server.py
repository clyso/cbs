# CBS - config - server config
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.

from __future__ import annotations

import os
from pathlib import Path
from typing import Annotated

import pydantic
from cbslib.logger import log as parent_logger
from ceslib.errors import CESError
from fastapi import Depends

log = parent_logger.getChild("config")


# google oauth2 client secrets config
#
class GoogleOAuthSecrets(pydantic.BaseModel):
    project_id: str
    client_id: str
    client_secret: str
    auth_uri: str
    token_uri: str
    auth_provider_x509_cert_url: str
    redirect_uris: list[str]


class _GoogleOAuthSecrets(pydantic.BaseModel):
    web: GoogleOAuthSecrets

    @classmethod
    def load(cls, path: Path) -> GoogleOAuthSecrets:
        if not path.exists():
            raise CESError(f"oauth2 config not found at '{path}'")

        try:
            with path.open("r") as f:
                contents = _GoogleOAuthSecrets.model_validate_json(f.read())
            return contents.web
        except pydantic.ValidationError:
            raise CESError(f"malformed oauth2 config at '{path}'")
        except Exception as e:
            raise CESError(f"error loading oauth2 config from '{path}': {e}")


class ServerSecretsConfig(pydantic.BaseModel):
    # secrets generated with
    #   openssl rand -hex 32
    session_secret_key: str
    token_secret_key: str
    token_secret_ttl_minutes: int


# config
#
class SecretsConfig(pydantic.BaseModel):
    oauth2_secrets_file: str
    server: ServerSecretsConfig



class Config(pydantic.BaseModel):
    secrets: SecretsConfig

    # ssl certs
    #
    cert_path: Path
    key_path: Path

    # db path
    #
    db_path: Path

    @classmethod
    def load(cls, *, path: Path | None = None) -> Config:
        env_conf = os.getenv("CBS_CONFIG")
        env_conf_path = Path(env_conf) if env_conf else None
        config_path = path if path else env_conf_path
        if not config_path:
            raise CESError("missing config")

        if not config_path.exists():
            raise CESError(f"config at '{config_path}' does not exist")

        with config_path.open("r") as f:
            try:
                return Config.model_validate_json(f.read())
            except pydantic.ValidationError:
                raise CESError(f"malformed config at '{config_path}'")
            except Exception as e:
                raise CESError(
                    f"unexpected error loading config at '{config_path}': {e}"
                )

    def get_oauth_config(self) -> GoogleOAuthSecrets:
        return _GoogleOAuthSecrets.load(Path(self.secrets.oauth2_secrets_file))


_config: Config | None = None


def config_init() -> Config:
    global _config
    if _config:
        return _config

    _config = Config.load()
    return _config


def cbs_config() -> Config:
    if not _config:
        raise CESError("config not set!")
    return _config


def get_config() -> Config:
    if not _config:
        raise CESError("config not set!")
    return _config.model_copy(deep=True)


CBSConfig = Annotated[Config, Depends(cbs_config)]
