# CES library - images signing
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

import os
from typing import override

from cbscore.errors import CESError
from cbscore.images import logger as parent_logger
from cbscore.utils import CmdArgs, CommandError, PasswordArg, async_run_cmd, run_cmd
from cbscore.utils.secrets import SecretsVaultError, SecretsVaultMgr

logger = parent_logger.getChild("sign")


class SigningError(CESError):
    @override
    def __str__(self) -> str:
        return f"Signing Error: {self.msg}"


def sign(img: str, secrets: SecretsVaultMgr) -> tuple[int, str, str]:
    try:
        _, username, password = secrets.harbor_creds()
    except SecretsVaultError as e:
        logger.exception("error obtaining harbor credentials")
        raise e  # noqa: TRY201

    cmd: CmdArgs = [
        "cosign",
        "sign",
        "--key=hashivault://container-image-key",
        PasswordArg("--registry-username", username),
        PasswordArg("--registry-password", password),
        "--tlog-upload=false",
        "--upload=true",
        img,
    ]

    vault_transit = secrets.vault.transit
    assert vault_transit is not None

    with secrets.vault.client() as client:
        vault_token = client.token

    env = os.environ.copy()
    env.update(
        {
            "VAULT_ADDR": secrets.vault.addr,
            "VAULT_TOKEN": vault_token,
            "TRANSIT_SECRET_ENGINE_PATH": vault_transit,
        }
    )
    return run_cmd(cmd, env=env)


async def async_sign(img: str, secrets: SecretsVaultMgr) -> None:
    def _out(s: str) -> None:
        logger.debug(s)

    try:
        _, username, password = secrets.harbor_creds()
    except SecretsVaultError as e:
        msg = f"error obtaining harbor credentials: {e}"
        logger.exception(msg)
        raise SigningError(msg) from e

    cmd: CmdArgs = [
        "cosign",
        "sign",
        "--key=hashivault://container-image-key",
        PasswordArg("--registry-username", username),
        PasswordArg("--registry-password", password),
        "--tlog-upload=false",
        "--upload=true",
        img,
    ]

    vault_transit = secrets.vault.transit
    if not vault_transit:
        msg = "vault transit unset, can't sign"
        logger.error(msg)
        raise SigningError(msg)

    with secrets.vault.client() as client:
        vault_token = client.token

    env = {
        "VAULT_ADDR": secrets.vault.addr,
        "VAULT_TOKEN": vault_token,
        "TRANSIT_SECRET_ENGINE_PATH": vault_transit,
    }

    try:
        rc, _, stderr = await async_run_cmd(cmd, outcb=_out, extra_env=env)
    except (CommandError, Exception) as e:
        msg = f"error signing image '{img}': {e}"
        logger.exception(msg)
        raise SigningError(msg) from e

    if rc != 0:
        msg = f"error signing image '{img}': {stderr}"
        logger.error(msg)
        raise SigningError(msg)
