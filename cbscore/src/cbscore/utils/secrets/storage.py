# CES library - secrets utilities - secrets manager (storage)
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

from cbscore.utils.secrets import logger as parent_logger
from cbscore.utils.secrets.models import (
    StoragePlainS3Secret,
    StorageSecret,
    StorageVaultS3Secret,
)
from cbscore.utils.vault import Vault

logger = parent_logger.getChild("storage")


def storage_get_s3_creds(
    host: str, secrets: dict[str, StorageSecret], vault: Vault | None
) -> tuple[str, str, str]:
    """
    Obtain s3 credentials for a given host.

    Returns a tuple with (host, access-id, secret-id).
    """
    entry = secrets.get(host)
    if not entry:
        msg = f"storage secret '{id}' not found"
        logger.error(msg)
        raise ValueError(msg)

    if isinstance(entry, StoragePlainS3Secret):
        return (host, entry.access_id, entry.secret_id)
    else:  # StorageVaultS3Secret
        assert isinstance(entry, StorageVaultS3Secret)
        if not vault:
            msg = f"storage secret '{id}' requires vault, but no vault configured"
            logger.error(msg)
            raise ValueError(msg)

        try:
            secret_data = vault.read_secret(entry.key)
            access_id = secret_data[entry.access_id]
            secret_id = secret_data[entry.secret_id]
        except Exception as e:
            msg = f"error retrieving storage secret '{id}' from vault: {e}"
            logger.error(msg)
            raise ValueError(msg) from e
        return (host, access_id.rstrip(), secret_id.rstrip())
