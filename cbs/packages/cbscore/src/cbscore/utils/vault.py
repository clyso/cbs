# CES library - hashicorp vault utilities
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

import abc
from collections.abc import Generator
from contextlib import contextmanager
from typing import override

import hvac
import hvac.exceptions

from cbscore.config import VaultConfig
from cbscore.errors import CESError
from cbscore.utils import logger as parent_logger

logger = parent_logger.getChild("vault")


class VaultError(CESError):
    @override
    def __str__(self) -> str:
        return f"Vault Error: {self.msg}"


# FIXME: vault backends currently login to vault on each client request.
# We should cache the token somehow.


class Vault(abc.ABC):
    addr: str
    transit: str | None

    def __init__(self, addr: str, *, transit: str | None = None) -> None:
        self.addr = addr
        if not self.addr:
            raise VaultError(msg="missing vault address")
        self.transit = transit

    @abc.abstractmethod
    @contextmanager
    def client(self) -> Generator[hvac.Client]:
        pass

    def read_secret(self, path: str) -> dict[str, str]:
        try:
            with self.client() as client:
                res = client.secrets.kv.v2.read_secret_version(
                    path=path,
                    mount_point="ces-kv",
                    raise_on_deleted_version=False,
                )
                logger.debug(f"obtained secret '{path}' from vault")
        except hvac.exceptions.Forbidden:
            raise VaultError(msg="permission denied obtaining secret") from None
        except Exception as e:
            raise VaultError(msg=f"error obtaining secret: {e}") from e

        try:
            entry = res["data"]["data"]
        except KeyError as e:
            raise VaultError(msg=f"error obtaining secret's entry: {e}") from None

        return entry


class VaultAppRoleBackend(Vault):
    role_id: str
    secret_id: str

    def __init__(
        self, addr: str, role_id: str, secret_id: str, *, transit: str | None = None
    ) -> None:
        super().__init__(addr, transit=transit)
        if not role_id:
            raise VaultError(msg="missing role id")
        if not secret_id:
            raise VaultError(msg="missing secret id")
        self.role_id = role_id
        self.secret_id = secret_id

    @override
    @contextmanager
    def client(self) -> Generator[hvac.Client]:
        client = hvac.Client(url=self.addr)
        try:
            client.auth.approle.login(
                role_id=self.role_id,
                secret_id=self.secret_id,
                use_token=True,
            )
            logger.debug("approle logged in to vault")
        except hvac.exceptions.Forbidden:
            raise VaultError(msg="permission denied logging in to vault") from None
        except Exception:
            raise VaultError(msg="error logging in to vault") from None

        yield client


class VaultUserPassBackend(Vault):
    username: str
    password: str

    def __init__(
        self, addr: str, username: str, password: str, *, transit: str | None = None
    ) -> None:
        super().__init__(addr, transit=transit)
        if not username:
            raise VaultError(msg="missing username")
        if not password:
            raise VaultError(msg="missing password")
        self.username = username
        self.password = password

    @override
    @contextmanager
    def client(self) -> Generator[hvac.Client]:
        client = hvac.Client(url=self.addr)
        try:
            client.auth.userpass.login(
                username=self.username, password=self.password, use_token=True
            )
            logger.debug("userpass logged in to vault")
        except hvac.exceptions.Forbidden:
            raise VaultError(msg="permission denied logging in to vault") from None
        except Exception:
            raise VaultError(msg="error logging in to vault") from None

        yield client


class VaultTokenBackend(Vault):
    token: str

    def __init__(self, addr: str, token: str, *, transit: str | None = None) -> None:
        super().__init__(addr, transit=transit)
        if not token:
            raise VaultError(msg="missing token")
        self.token = token

    @override
    @contextmanager
    def client(self) -> Generator[hvac.Client]:
        client = hvac.Client(url=self.addr, token=self.token)
        yield client


def get_vault_from_config(vault_config: VaultConfig) -> Vault:
    if vault_config.auth_approle:
        return VaultAppRoleBackend(
            addr=vault_config.vault_addr,
            role_id=vault_config.auth_approle.role_id,
            secret_id=vault_config.auth_approle.secret_id,
            transit=vault_config.vault_transit,
        )
    elif vault_config.auth_user:
        return VaultUserPassBackend(
            addr=vault_config.vault_addr,
            username=vault_config.auth_user.username,
            password=vault_config.auth_user.password,
            transit=vault_config.vault_transit,
        )
    elif vault_config.auth_token:
        return VaultTokenBackend(
            addr=vault_config.vault_addr,
            token=vault_config.auth_token,
            transit=vault_config.vault_transit,
        )
    else:
        raise VaultError(msg="no authentication method configured for vault")
