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
        if not path.exists():
            raise CESError(msg=f"oauth2 config not found at '{path}'")

        try:
            with path.open("r") as f:
                contents = _GoogleOAuthSecrets.model_validate_json(f.read())
        except pydantic.ValidationError:
            raise CESError(msg=f"malformed oauth2 config at '{path}'") from None
        except Exception as e:
            raise CESError(msg=f"error loading oauth2 config from '{path}': {e}") from e
        return contents.web


class ServerSecretsConfig(pydantic.BaseModel):
    oauth2_secrets_file: str
    # secrets generated with
    #   openssl rand -hex 32
    session_secret_key: str
    token_secret_key: str
    token_secret_ttl_minutes: int


class ServerConfig(pydantic.BaseModel):
    # ssl certs
    #
    cert_path: Path
    key_path: Path

    # database path
    #
    db_path: Path

    # server secrets
    #
    secrets: ServerSecretsConfig

    def get_oauth_config(self) -> GoogleOAuthSecrets:
        return _GoogleOAuthSecrets.load(Path(self.secrets.oauth2_secrets_file))
