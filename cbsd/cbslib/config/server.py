# CBS server library - config - server
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

from pathlib import Path
from typing import Annotated, ClassVar

import pydantic
from cbscore.errors import CESError

from cbslib.logger import logger as parent_logger

logger = parent_logger.getChild("config")


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
        if not path.exists() or not path.is_file():
            msg = f"oauth2 config file not found at '{path}'"
            logger.error(msg)
            raise CESError(msg)

        if path.suffix.lower() != ".json":
            msg = f"oauth2 config file at '{path}' is not a json file"
            logger.error(msg)
            raise CESError(msg)

        try:
            contents = _GoogleOAuthSecrets.model_validate_json(path.read_text())
        except pydantic.ValidationError as e:
            msg = f"malformed oauth2 config at '{path}': {e}"
            logger.error(msg)
            raise CESError(msg) from None
        except Exception as e:
            msg = f"unexpected error loading oauth2 config from '{path}': {e}"
            logger.error(msg)
            raise CESError(msg) from e
        return contents.web


class ServerSecretsConfig(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        populate_by_name=True,
        validate_by_alias=True,
        serialize_by_alias=True,
    )

    oauth2_secrets_file: Annotated[str, pydantic.Field(alias="oauth2-secrets-file")]
    # secrets generated with
    #   openssl rand -hex 32
    session_secret_key: Annotated[str, pydantic.Field(alias="session-secret-key")]
    token_secret_key: Annotated[str, pydantic.Field(alias="token-secret-key")]
    token_secret_ttl_minutes: Annotated[
        int, pydantic.Field(alias="token-secret-ttl-minutes")
    ]


class ServerConfig(pydantic.BaseModel):
    # ssl certs
    #
    cert: Path
    key: Path

    # database path
    #
    db: Path

    # permissions file
    #
    permissions: Path

    # server secrets
    #
    secrets: ServerSecretsConfig

    # logs file path
    #
    logs: Path

    def get_oauth_config(self) -> GoogleOAuthSecrets:
        return _GoogleOAuthSecrets.load(Path(self.secrets.oauth2_secrets_file))
