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

from collections.abc import Generator
from contextlib import contextmanager
from typing import override

import hvac
import hvac.exceptions
from ceslib.errors import CESError
from ceslib.utils import log as parent_logger

log = parent_logger.getChild("vault")


class VaultError(CESError):
    @override
    def __str__(self) -> str:
        return f"Vault Error: {self.msg}"


class Vault:
    addr: str
    transit: str | None
    role_id: str
    secret_id: str

    def __init__(
        self, addr: str, role_id: str, secret_id: str, *, transit: str | None = None
    ) -> None:
        self.addr = addr
        if not self.addr:
            raise VaultError("missing vault address")
        if not role_id:
            raise VaultError("missing role id")
        if not secret_id:
            raise VaultError("missing secret id")
        self.transit = transit
        self.role_id = role_id
        self.secret_id = secret_id

    @contextmanager
    def client(self) -> Generator[hvac.Client]:
        client = hvac.Client(url=self.addr)
        try:
            client.auth.approle.login(
                role_id=self.role_id,
                secret_id=self.secret_id,
                use_token=True,
            )
            log.info("logged in to vault")
        except hvac.exceptions.Forbidden:
            raise VaultError("permission denied logging in to vault")
        except Exception:
            raise VaultError("error logging in to vault")

        yield client

    def read_secret(self, path: str) -> dict[str, str]:
        try:
            with self.client() as client:
                res = client.secrets.kv.v2.read_secret_version(
                    path=path,
                    mount_point="ces-kv",
                    raise_on_deleted_version=False,
                )
                log.debug(f"obtained secret '{path}' from vault")
        except hvac.exceptions.Forbidden:
            raise VaultError("permission denied obtaining secret")
        except Exception as e:
            raise VaultError(f"error obtaining secret: {e}")

        try:
            entry = res["data"]["data"]
        except KeyError as e:
            raise VaultError(f"error obtaining secret's entry: {e}")

        return entry
