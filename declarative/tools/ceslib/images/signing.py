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

from ceslib.images import log as parent_logger
from ceslib.utils import run_cmd
from ceslib.utils.secrets import SecretsVaultError, SecretsVaultMgr

log = parent_logger.getChild("sign")


def sign(img: str, secrets: SecretsVaultMgr) -> tuple[int, str, str]:
    try:
        _, username, password = secrets.harbor_creds()
    except SecretsVaultError as e:
        log.error(f"error obtaining harbor credentials: {e}")
        raise e

    cmd = [
        "cosign",
        "sign",
        "--key=hashivault://container-image-key",
        f"--registry-username={username}",
        f"--registry-password={password}",
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
