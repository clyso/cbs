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
from ceslib.images.auth import AuthAndSignInfo
from ceslib.utils import run_cmd


def sign(img: str, auth_info: AuthAndSignInfo) -> tuple[int, str, str]:
    cmd = [
        "cosign",
        "sign",
        "--key=hashivault://container-image-key",
        f"--registry-username={auth_info.harbor_username}",
        f"--registry-password={auth_info.harbor_password}",
        "--tlog-upload=false",
        "--upload=true",
        img,
    ]
    env = os.environ.copy()
    env.update(
        {
            "VAULT_ADDR": auth_info.vault_addr,
            "VAULT_TOKEN": auth_info.vault_token,
            "TRANSIT_SECRET_ENGINE_PATH": auth_info.vault_transit,
        }
    )
    return run_cmd(cmd, env=env)
