# CES library - images authentication utilities
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

# pyright: reportUnknownMemberType=false
# pyright: reportExplicitAny=false
# pyright: reportUnknownVariableType=false

from typing import Any
import hvac
import hvac.exceptions

from ceslib.images import log as parent_logger
from ceslib.images.errors import AuthError

log = parent_logger.getChild("auth")


class AuthAndSignInfo:
    harbor_username: str
    harbor_password: str
    vault_addr: str
    vault_transit: str

    vault_client: hvac.Client

    def __init__(
        self,
        vault_addr: str,
        vault_role_id: str,
        vault_secret_id: str,
        vault_transit: str,
    ) -> None:
        self.vault_addr = vault_addr
        if self.vault_addr == "":
            raise AuthError("missing vault address")
        if vault_role_id == "":
            raise AuthError("missing vault role id")
        if vault_secret_id == "":
            raise AuthError("missing vault secret id")
        self.vault_transit = vault_transit
        if self.vault_transit == "":
            raise AuthError("missing vault transit")

        self.vault_login(vault_role_id, vault_secret_id)

    def vault_login(self, role_id: str, secret_id: str) -> None:
        self.vault_client = hvac.Client(url=self.vault_addr)

        try:
            self.vault_client.auth.approle.login(
                role_id=role_id,
                secret_id=secret_id,
                use_token=True,
            )
            log.info("logged in to vault")
        except hvac.exceptions.Forbidden:
            raise AuthError("permission denied logging in to vault")
        except Exception:
            raise AuthError("error logging in to vault")

        try:
            res: dict[str, Any] = self.vault_client.secrets.kv.v2.read_secret_version(
                path="creds/harbor.clyso.com:ces-build/ces-build-bot",
                mount_point="ces-kv",
                raise_on_deleted_version=False,
            )
            log.info("obtained harbor credentials from vault")
        except hvac.exceptions.Forbidden:
            raise AuthError("permission denied while obtainining harbor credentials")
        except Exception:
            raise AuthError("error obtaining harbor credentials")

        try:
            self.harbor_username = res["data"]["data"]["username"]
            self.harbor_password = res["data"]["data"]["password"]
        except KeyError as e:
            raise AuthError(f"missing key in harbor credentials: {e}")

        log.debug(
            f"harbor credentials: username = {self.harbor_username}, "
            + f"password = {self.harbor_password}"
        )

    @property
    def vault_token(self) -> str:
        return self.vault_client.token
