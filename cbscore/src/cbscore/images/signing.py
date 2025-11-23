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
from cbscore.utils import (
    CmdArgs,
    CommandError,
    PasswordArg,
    async_run_cmd,
    run_cmd,
)
from cbscore.utils.containers import get_container_image_base_uri
from cbscore.utils.secrets import SecretsMgrError
from cbscore.utils.secrets.mgr import SecretsMgr

logger = parent_logger.getChild("sign")


class SigningError(CESError):
    @override
    def __str__(self) -> str:
        return f"Signing Error: {self.msg}"


def sign(
    registry: str, img: str, secrets: SecretsMgr, transit: str
) -> tuple[int, str, str]:
    if not secrets.has_vault():
        msg = "no vault configured, can't sign image"
        logger.error(msg)
        raise SigningError(msg)

    assert secrets.vault is not None

    if not secrets.has_transit_key(transit):
        msg = f"vault transit key '{transit}' not found, can't sign image"
        logger.error(msg)
        raise SigningError(msg)

    try:
        _, username, password = secrets.registry_creds(registry)
    except SecretsMgrError as e:
        msg = f"unable to obtain registry credentials for '{registry}': {e}"
        logger.error(msg)
        raise SigningError(msg) from e

    try:
        transit_mount, transit_key = secrets.transit(transit)
    except SecretsMgrError as e:
        msg = f"unable to obtain transit key '{transit}': {e}"
        logger.error(msg)
        raise SigningError(msg) from e

    cmd: CmdArgs = [
        "cosign",
        "sign",
        f"--key=hashivault://{transit_key}",
        PasswordArg("--registry-username", username),
        PasswordArg("--registry-password", password),
        "--tlog-upload=false",
        "--upload=true",
        img,
    ]

    with secrets.vault.client() as client:
        vault_token = client.token

    env = os.environ.copy()
    env.update(
        {
            "VAULT_ADDR": secrets.vault.addr,
            "VAULT_TOKEN": vault_token,
            "TRANSIT_SECRET_ENGINE_PATH": transit_mount,
        }
    )
    return run_cmd(cmd, env=env)


async def async_sign(img: str, secrets: SecretsMgr, transit: str) -> None:
    def _out(s: str) -> None:
        logger.debug(s)

    if not secrets.has_vault():
        msg = "no vault configured, can't sign image"
        logger.error(msg)
        raise SigningError(msg)

    assert secrets.vault is not None

    if not secrets.has_transit_key(transit):
        msg = f"vault transit key '{transit}' not found, can't sign image"
        logger.error(msg)
        raise SigningError(msg)

    try:
        img_uri = get_container_image_base_uri(img)
    except ValueError as e:
        msg = f"error obtaining image base URI: {e}"
        logger.error(msg)
        raise SigningError(msg) from e

    try:
        _, username, password = secrets.registry_creds(img_uri)
    except ValueError as e:
        logger.warning(f"unable to obtain registry credentials for '{img_uri}': {e}")
        logger.warning("assume unauthenticated registry access")
        username = password = ""
    except SecretsMgrError as e:
        msg = f"error obtaining registry credentials for '{img_uri}': {e}"
        logger.error(msg)
        raise SigningError(msg) from e

    try:
        transit_mount, transit_key = secrets.transit(transit)
    except SecretsMgrError as e:
        msg = f"unable to obtain transit key '{transit}': {e}"
        logger.error(msg)
        raise SigningError(msg) from e

    cmd: CmdArgs = ["cosign", "sign", f"--key=hashivault://{transit_key}"]
    cmd.extend(
        [
            PasswordArg("--registry-username", username),
            PasswordArg("--registry-password", password),
        ]
        if username and password
        else []
    )
    cmd.extend(
        [
            "--tlog-upload=false",
            "--upload=true",
            img,
        ]
    )

    with secrets.vault.client() as client:
        vault_token = client.token

    env = os.environ.copy()
    env.update(
        {
            "VAULT_ADDR": secrets.vault.addr,
            "VAULT_TOKEN": vault_token,
            "TRANSIT_SECRET_ENGINE_PATH": transit_mount,
        }
    )

    try:
        rc, _, stderr = await async_run_cmd(cmd, outcb=_out, extra_env=env)
    except (CommandError, Exception) as e:
        msg = f"error signing image '{img}': {e}"
        logger.error(msg)
        raise SigningError(msg) from e

    if rc != 0:
        msg = f"error signing image '{img}': {stderr}"
        logger.error(msg)
        raise SigningError(msg)
