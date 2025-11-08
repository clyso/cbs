# CES library - secrets utilities - secrets manager
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


import errno
import sys
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path

from cbscore.config import Config, ConfigError, VaultConfig
from cbscore.utils import MaybeSecure
from cbscore.utils.secrets import SecretsMgrError
from cbscore.utils.secrets import logger as parent_logger
from cbscore.utils.secrets.git import git_url_for
from cbscore.utils.secrets.models import (
    GPGPlainSecret,
    GPGVaultPrivateKeySecret,
    GPGVaultSingleSecret,
    Secrets,
    VaultTransitSecret,
)
from cbscore.utils.secrets.registry import registry_get_creds
from cbscore.utils.secrets.signing import gpg_private_keyring, signing_transit
from cbscore.utils.secrets.storage import storage_get_s3_creds
from cbscore.utils.vault import Vault, VaultError, get_vault_from_config

logger = parent_logger.getChild("mgr")


class SecretsMgr:
    """Handle secrets management for cbscore."""

    vault: Vault | None
    secrets: Secrets

    def __init__(
        self, secrets: Secrets, *, vault_config: VaultConfig | None = None
    ) -> None:
        try:
            self.vault = get_vault_from_config(vault_config) if vault_config else None
        except VaultError as e:
            msg = f"error obtaining vault config: {e}"
            logger.error(msg)
            raise SecretsMgrError(msg) from e

        # propagate exceptions, if any.
        self.secrets = secrets

        if self.vault:
            try:
                self.vault.check_vault_connection()
            except VaultError as e:
                msg = f"error connecting to vault: {e}"
                logger.error(msg)
                raise SecretsMgrError(msg) from e

    @contextmanager
    def git_url_for(self, url: str) -> Generator[MaybeSecure]:
        """Obtain git url with credentials for specified URL, if any."""
        with git_url_for(url, self.secrets.git, self.vault) as git_url:
            yield git_url

    def s3_creds(self, url: str) -> tuple[str, str, str]:
        """Obtain S3 credentials for the specified URL, if any."""
        return storage_get_s3_creds(url, self.secrets.storage, self.vault)

    @contextmanager
    def gpg_signing_key(self, id: str) -> Generator[tuple[Path, str | None, str]]:
        """
        Obtain GPG signing key, if any.

        Returns a tuple containing the path to the keyring containing the key, along
        with the key's passphrase and its email (if any).
        """
        with gpg_private_keyring(id, self.secrets.sign, self.vault) as gpg_key:
            yield gpg_key

    def transit(self, id: str) -> tuple[str, str]:
        """Obtain transit key information for the specified ID, if any."""
        return signing_transit(id, self.secrets.sign)

    def registry_creds(self, id: str) -> tuple[str, str, str]:
        """Obtain registry credentials for the specified registry ID, if any."""
        return registry_get_creds(id, self.secrets.registry, self.vault)

    def has_vault(self) -> bool:
        """Check whether a vault is configured."""
        return self.vault is not None

    def has_s3_creds(self, url: str) -> bool:
        return self.secrets.storage.get(url) is not None

    def has_gpg_signing_key(self, id: str) -> bool:
        """Check whether a signing key with the specified ID exists."""
        secret = self.secrets.sign.get(id)
        return secret is not None and isinstance(
            secret, GPGPlainSecret | GPGVaultSingleSecret | GPGVaultPrivateKeySecret
        )

    def has_transit_key(self, id: str) -> bool:
        """Check whether a transit key with the specified ID exists."""
        secret = self.secrets.sign.get(id)
        return secret is not None and isinstance(secret, VaultTransitSecret)

    def has_registry_creds(self, id: str) -> bool:
        """Check whether registry credentials with the specified ID exist."""
        return self.secrets.registry.get(id) is not None


#
# kludge to test obtaining secrets.
#
check_mark = "\u2714"  # ✔
error_mark = "\u274c"  # ❌


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <config-path>")
        sys.exit(errno.EINVAL)

    try:
        config = Config.load(Path(sys.argv[1]))
    except ConfigError as e:
        print(f"error loading config: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    secrets = config.get_secrets()
    if not secrets:
        print("no secrets defined in config")
        sys.exit(errno.EINVAL)
    mgr = SecretsMgr(secrets)

    try:
        with mgr.git_url_for("https://example.com/repo.git") as git_url:
            print(
                f"{check_mark} found git url for example repo (must be same): {git_url}"
            )
    except SecretsMgrError:
        print(f"{check_mark} incorrectly did not find git url for example repo")

    try:
        with mgr.git_url_for("https://github.com/ceph/foo") as git_url:
            print(f"{check_mark} found git url for github.com/ceph/foo: {git_url}")
    except SecretsMgrError as e:
        print(f"{error_mark} error obtaining git url for github.com/ceph/foo: {e}")

    try:
        with mgr.git_url_for("https://gitlab.foo.tld/ceph/ceph") as git_url:
            print(f"{check_mark} found git url for gitlab.foo.tld/ceph/ceph: {git_url}")
    except SecretsMgrError as e:
        print(f"{error_mark} error obtaining git url for gitlab.foo.tld/ceph/ceph: {e}")
