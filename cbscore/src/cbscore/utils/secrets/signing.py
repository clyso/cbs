# CES library - secrets utilities - secrets manager (signing)
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
import random
import shutil
import subprocess
import tempfile
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path

from cbscore.utils.secrets import SecretsMgrError
from cbscore.utils.secrets import logger as parent_logger
from cbscore.utils.secrets.models import (
    GPGPlainSecret,
    GPGVaultPrivateKeySecret,
    GPGVaultSingleSecret,
    SigningSecret,
    VaultTransitSecret,
)
from cbscore.utils.vault import Vault, VaultError

logger = parent_logger.getChild("signing")


def _get_gpg_private_key_from(
    entry: GPGPlainSecret | GPGVaultSingleSecret | GPGVaultPrivateKeySecret,
    vault: Vault | None,
) -> tuple[str, str | None, str]:
    if isinstance(entry, GPGPlainSecret):
        return (entry.private_key, entry.passphrase, entry.email)

    gpg_pvt_key: str
    gpg_passphrase: str | None

    if not vault:
        msg = f"vault is required to obtain GPG private key from '{entry}'"
        logger.error(msg)
        raise SecretsMgrError(msg)

    try:
        gpg_secret = vault.read_secret(entry.key)
    except VaultError as e:
        msg = f"error obtaining GPG private key from vault: {e}"
        logger.error(msg)
        raise SecretsMgrError(msg) from e

    try:
        gpg_pvt_key = gpg_secret[entry.private_key]
        gpg_passphrase = gpg_secret[entry.passphrase] if entry.passphrase else None
        gpg_email = gpg_secret[entry.email]
    except KeyError as e:
        msg = f"error obtaining GPG private key credentials: {e}"
        logger.error(msg)
        raise SecretsMgrError(msg) from e

    return (
        gpg_pvt_key.rstrip(),
        gpg_passphrase.rstrip() if gpg_passphrase else None,
        gpg_email.rstrip(),
    )


@contextmanager
def _get_gpg_private_key_file(
    entry: GPGPlainSecret | GPGVaultSingleSecret | GPGVaultPrivateKeySecret,
    vault: Vault | None,
) -> Generator[tuple[Path, str | None, str]]:
    gpg_pvt_key, gpg_passphrase, gpg_email = _get_gpg_private_key_from(entry, vault)

    try:
        _, gpg_pvt_file = tempfile.mkstemp()
        gpg_pvt_path = Path(gpg_pvt_file)
        with gpg_pvt_path.open("w") as f:
            n = f.write(gpg_pvt_key)
    except Exception as e:
        msg = f"error writing gpg private key to file: {e}"
        logger.error(msg)
        raise SecretsMgrError(msg) from e

    yield gpg_pvt_path, gpg_passphrase, gpg_email

    try:
        with gpg_pvt_path.open("bw") as f:
            _ = f.write(random.randbytes(n))  # noqa: S311
        gpg_pvt_path.unlink()
    except Exception as e:
        msg = f"error cleaning up gpg private key file: {e}"
        logger.error(msg)
        raise SecretsMgrError(msg) from e


@contextmanager
def _get_gpg_private_keyring(
    entry: GPGPlainSecret | GPGVaultSingleSecret | GPGVaultPrivateKeySecret,
    vault: Vault | None,
) -> Generator[tuple[Path, str | None, str]]:
    with _get_gpg_private_key_file(entry, vault) as pvt:
        logger.debug("obtained gpg private key")
        pvt_key_file = pvt[0]  # Path to private key file
        passphrase = pvt[1]  # private key's passphrase (if any)
        email = pvt[2]  # private key's email (if any)

        keyring_path = Path(tempfile.mkdtemp())
        keyring_path.chmod(0o700)

        logger.debug(f"import gpg private key from '{pvt_key_file}'")
        cmd = ["gpg", "--import", "--batch", pvt_key_file.resolve().as_posix()]
        env = os.environ.copy()
        env["GNUPGHOME"] = keyring_path.resolve().as_posix()
        try:
            p = subprocess.run(cmd, capture_output=True, env=env)  # noqa: S603
            logger.debug(f"stdout: {p.stdout}")
            logger.debug(f"stderr: {p.stderr}")
        except Exception as e:
            msg = f"error importing gpg private key: {e}"
            logger.error(msg)
            raise SecretsMgrError(msg) from e

        if p.returncode != 0:
            msg = f"error importing gpg private key from '{pvt_key_file}': {p.stderr}"
            logger.error(msg)
            raise SecretsMgrError(msg)

        logger.debug(f"return keyring at '{keyring_path}', " + f"email '{email}'")

        yield keyring_path, passphrase, email

        try:
            shutil.rmtree(keyring_path)
        except Exception as e:
            msg = f"error cleaning up keyring at '{keyring_path}': {e}"
            logger.error(msg)
            raise SecretsMgrError(msg) from e


@contextmanager
def gpg_private_keyring(
    id: str, secrets: dict[str, SigningSecret], vault: Vault | None
) -> Generator[tuple[Path, str | None, str]]:
    """Obtain GPG private keyring for signing secret with specified ID."""
    secret = secrets.get(id)
    if not secret:
        msg = f"signing secret with ID '{id}' not found"
        logger.error(msg)
        raise SecretsMgrError(msg)

    if not isinstance(
        secret,
        GPGPlainSecret | GPGVaultSingleSecret | GPGVaultPrivateKeySecret,
    ):
        msg = f"signing secret with ID '{id}' is not a GPG private key"
        logger.error(msg)
        raise SecretsMgrError(msg)

    with _get_gpg_private_keyring(secret, vault) as keyring:
        yield keyring


def signing_transit(id: str, secrets: dict[str, SigningSecret]) -> tuple[str, str]:
    """Obtain Vault Transit signing key information for the specified ID, if any."""
    secret = secrets.get(id)
    if not secret:
        msg = f"signing secret with ID '{id}' not found"
        logger.error(msg)
        raise SecretsMgrError(msg)

    if not isinstance(secret, VaultTransitSecret):
        msg = f"signing secret with ID '{id}' is not a Vault Transit secret"
        logger.error(msg)
        raise SecretsMgrError(msg)

    return (secret.mount, secret.key)
