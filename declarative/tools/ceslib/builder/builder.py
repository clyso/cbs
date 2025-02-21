# CES library - CES builder
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

from pathlib import Path

from ceslib.builder import BuilderError
from ceslib.builder import log as parent_logger
from ceslib.builder.prepare import prepare
from ceslib.builder.rpmbuild import build_rpms
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.utils.vault import VaultError
from ceslib.versions.desc import VersionDescriptor

log = parent_logger.getChild("builder")


class Builder:
    desc: VersionDescriptor
    scratch_path: Path
    components_path: Path
    upload: bool
    secrets: SecretsVaultMgr

    def __init__(
        self,
        desc: VersionDescriptor,
        vault_addr: str,
        vault_role_id: str,
        vault_secret_id: str,
        scratch_path: Path,
        secrets_path: Path,
        components_path: Path,
        upload: bool,
    ) -> None:
        self.desc = desc
        self.scratch_path = scratch_path
        self.components_path = components_path
        self.upload = upload

        try:
            self.secrets = SecretsVaultMgr(
                secrets_path, vault_addr, vault_role_id, vault_secret_id
            )
        except VaultError as e:
            log.error(f"error logging in to vault: {e}")
            raise BuilderError(f"error logging in to vault: {e}")

    async def run(self) -> None:
        log.info("preparing builder")

        try:
            components = await prepare(
                self.desc, self.secrets, self.scratch_path, self.components_path
            )
        except BuilderError as e:
            raise BuilderError(f"error preparing builder: {e}")

        log.info("build RPMs")
        rpms_path = self.scratch_path.joinpath("rpms")
        rpms_path.mkdir(exist_ok=True)

        await build_rpms(
            rpms_path, self.desc.el_version, self.components_path, components
        )

        pass

    pass
