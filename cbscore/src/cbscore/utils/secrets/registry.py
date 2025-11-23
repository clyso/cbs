# CES library - secrets utilities - secrets manager (registry)
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

from cbscore.utils.secrets import SecretsMgrError
from cbscore.utils.secrets import logger as parent_logger
from cbscore.utils.secrets.models import (
    RegistryPlainSecret,
    RegistrySecret,
    RegistryVaultSecret,
)
from cbscore.utils.secrets.utils import find_best_secret_candidate
from cbscore.utils.vault import Vault, VaultError

logger = parent_logger.getChild("registry")


def _get_registry_from_vault(
    secret: RegistryVaultSecret, vault: Vault
) -> tuple[str, str, str]:
    """Retrieve registry credentials from the vault."""
    try:
        vault_secret = vault.read_secret(secret.key)
    except VaultError as e:
        msg = f"error obtaining registry vault secret '{secret.key}': {e}"
        logger.error(msg)
        raise SecretsMgrError(msg) from e

    try:
        username = vault_secret[secret.username]
        password = vault_secret[secret.password]
        address = vault_secret[secret.address]
    except KeyError as e:
        msg = f"missing field in registry vault secret '{secret.key}': {e}"
        logger.error(msg)
        raise SecretsMgrError(msg) from e

    return (address.rstrip(), username.rstrip(), password.rstrip())


def registry_get_creds(
    uri: str,
    secrets: dict[str, RegistrySecret],
    vault: Vault | None,
) -> tuple[str, str, str]:
    """
    Obtain registry credentials for a given id.

    Returns a tuple with (address, username, password).
    """
    best_entry = find_best_secret_candidate(list(secrets.keys()), uri)
    if not best_entry:
        msg = f"secret for uri '{uri}' not found"
        logger.warning(msg)
        raise ValueError(msg)

    secret = secrets[best_entry]
    assert isinstance(secret, RegistryPlainSecret | RegistryVaultSecret)

    if isinstance(secret, RegistryPlainSecret):
        return (secret.address, secret.username, secret.password)

    if not vault:
        msg = f"no vault configured for registry vault secret for '{uri}'"
        logger.error(msg)
        raise SecretsMgrError(msg)

    return _get_registry_from_vault(secret, vault)
