# CES library - secrets utilities
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

# NOTE: pydantic makes basedpyright complain about 'Any' when using Field
# defaults. Disable temporarily.
#
# pyright: reportAny=false, reportUnknownArgumentType=false

from __future__ import annotations

import logging
import os
import random
import re
import shutil
import string
import subprocess
import tempfile
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import override

import pydantic

from cbscore.config import VaultConfig
from cbscore.errors import CESError
from cbscore.utils import MaybeSecure, Password, SecureURL
from cbscore.utils import logger as parent_logger
from cbscore.utils.vault import Vault, VaultError, get_vault_from_config

logger = parent_logger.getChild("secrets")


class SecretsError(CESError):
    @override
    def __str__(self) -> str:
        return f"Secrets Error: {self.msg}"


class SecretsVaultError(CESError):
    @override
    def __str__(self) -> str:
        return f"Vault Secrets Error: {self.msg}"


class VaultSecrets(pydantic.BaseModel):
    vault_key: str


class GitSSHSecrets(VaultSecrets):
    ssh_key: str
    extras: dict[str, str] = pydantic.Field(default={})


class GitHTTPSSecrets(VaultSecrets):
    username: str
    password: str


class S3Secrets(VaultSecrets):
    hostname: str
    access_id: str
    secret_id: str


class GPGPublicKeySecrets(VaultSecrets):
    key: str


class GPGPrivateKeySecrets(VaultSecrets):
    key: str
    passphrase: str


class GPGExtras(pydantic.BaseModel):
    email: str


class GPGSecrets(pydantic.BaseModel):
    public: GPGPublicKeySecrets
    private: GPGPrivateKeySecrets
    extras: GPGExtras


class HarborSecrets(VaultSecrets):
    username: str
    password: str
    extras: dict[str, str] = pydantic.Field(default={})


class Secrets(pydantic.BaseModel):
    git: dict[str, GitSSHSecrets | GitHTTPSSecrets]
    s3: S3Secrets
    gpg: GPGSecrets
    harbor: HarborSecrets

    @classmethod
    def read(cls, path: Path) -> Secrets:
        if not path.exists() or not path.is_file():
            raise SecretsError(msg=f"credentials not found at '{path}'")

        with path.open("r") as f:
            raw_json = f.read()

        try:
            return Secrets.model_validate_json(raw_json)
        except pydantic.ValidationError:
            raise SecretsError(
                msg=f"error validating credentials at '{path}'"
            ) from None
        except Exception as e:
            raise SecretsError(msg=f"error validating credentials at '{path}'") from e


class GitSSHSecretCtx:
    vault: Vault
    secret: GitSSHSecrets

    def __init__(self, vault: Vault, secret: GitSSHSecrets) -> None:
        self.vault = vault
        self.secret = secret


class SecretsVaultMgr:
    vault: Vault
    secrets: Secrets
    log: logging.Logger

    def __init__(
        self,
        secrets_path: Path,
        vault_config: VaultConfig,
    ) -> None:
        # propagate errors, let caller deal with them
        self.vault = get_vault_from_config(vault_config)
        self.secrets = Secrets.read(secrets_path)
        self.log = logger.getChild("secrets-vault-mgr")

    @contextmanager
    def git_url_for(self, url: str) -> Generator[MaybeSecure]:
        entry: GitSSHSecrets | GitHTTPSSecrets | None = None
        pattern = re.compile(r"^(?:https:\/\/)?([^./]+(?:\.[^./]+)+(?:\/.*)?)$")
        for target, secrets_entry in self.secrets.git.items():
            if target in url:
                entry = secrets_entry
                break

        if entry is None:
            raise SecretsVaultError(msg=f"unable to find secret for '{url}'")

        m = re.match(pattern, url)
        if m is None:
            raise SecretsVaultError(msg=f"malformed url '{url}'")

        matched_url: str = m.group(1)

        if isinstance(entry, GitSSHSecrets):
            homedir = os.getenv("HOME")
            if homedir is None:
                raise SecretsVaultError(
                    msg="unable to obtain home directory for ssh key"
                )
            ssh_conf_dir = Path(homedir).joinpath(".ssh")
            ssh_conf_dir.mkdir(mode=0o700, parents=True, exist_ok=True)

            remote_name = "".join(
                random.choice(  # noqa: S311
                    string.ascii_letters
                )
                for _ in range(10)
            )

            # split matched url into git repo host and git repo
            idx = matched_url.find("/")
            if idx <= 0 or (len(matched_url) - 1) == idx:
                raise SecretsVaultError(
                    msg=f"malformed url for ssh git repository: {url}"
                )
            target_host = matched_url[:idx]
            target_repo = matched_url[idx + 1 :]

            # obtain target host key, stash it
            try:
                p = subprocess.run(  # noqa: S603
                    ["ssh-keyscan", "-t", "rsa", target_host],  # noqa: S607
                    capture_output=True,
                )
                assert p.stdout
            except Exception as e:
                raise SecretsVaultError(
                    msg=f"error obtaining host key for '{target_host}': {e}"
                ) from e

            if p.returncode != 0:
                raise SecretsVaultError(
                    msg=f"error obtaining host key for '{target_host}': "
                    + f"{p.stderr.decode('utf-8')}"
                )

            with ssh_conf_dir.joinpath("known_hosts").open("a") as f:
                _ = f.write(p.stdout.decode("utf-8"))

            # setup pvt key and ssh config
            try:
                ssh_secret = self.vault.read_secret(entry.vault_key)
            except VaultError as e:
                raise SecretsVaultError(
                    msg=f"error obtaining ssh secret from vault: {e}"
                ) from e
            try:
                ssh_key = ssh_secret[entry.ssh_key]
            except KeyError as e:
                raise SecretsVaultError(msg=f"error obtaining ssh key: {e}") from e

            ssh_key_path = ssh_conf_dir.joinpath(f"{remote_name}.id")
            with ssh_key_path.open("w") as f:
                _ = f.write(ssh_key)
                _ = f.write("\n")
            ssh_key_path.chmod(0o600)

            ssh_username = entry.extras.get("username", "git")

            ssh_host_config = f"""
Host {remote_name}
    Hostname {target_host}
    User {ssh_username}
    IdentityFile {ssh_key_path.as_posix()}

"""
            ssh_conf_path = ssh_conf_dir.joinpath("config")
            with ssh_conf_path.open("a") as f:
                _ = f.write(ssh_host_config)

            ssh_url = f"{remote_name}:{target_repo}"
            yield ssh_url

            # clean up private key
            ssh_key_path.unlink()

        else:  # https git repository
            try:
                https_secret = self.vault.read_secret(entry.vault_key)
            except VaultError as e:
                raise SecretsVaultError(
                    msg=f"error obtaining https credentials from vault: {e}"
                ) from e

            try:
                username = https_secret[entry.username].rstrip()
                password = https_secret[entry.password].rstrip()
            except KeyError as e:
                raise SecretsVaultError(
                    msg=f"error obtaining https credentials: {e}"
                ) from e

            https_secure_url = SecureURL(
                "https://{username}:{password}@{url}",
                username=username,
                password=Password(password),
                url=matched_url,
            )
            yield https_secure_url

    def harbor_creds(self) -> tuple[str, str, str]:
        try:
            harbor_secret = self.vault.read_secret(self.secrets.harbor.vault_key)
        except VaultError as e:
            raise SecretsVaultError(
                msg=f"error obtaining harbor credentials from vault: {e}"
            ) from e

        try:
            username = harbor_secret[self.secrets.harbor.username]
            password = harbor_secret[self.secrets.harbor.password]
            address = self.secrets.harbor.extras["address"]
        except KeyError as e:
            raise SecretsVaultError(
                msg=f"error obtaining harbor credentials: {e}"
            ) from e

        return address.rstrip(), username.rstrip(), password.rstrip()

    def s3_creds(self) -> tuple[str, str, str]:
        try:
            s3_secret = self.vault.read_secret(self.secrets.s3.vault_key)
        except VaultError as e:
            raise SecretsVaultError(
                msg=f"error obtaining S3 credentials from vault: {e}"
            ) from e

        try:
            hostname = s3_secret[self.secrets.s3.hostname]
            access_id = s3_secret[self.secrets.s3.access_id]
            secret_id = s3_secret[self.secrets.s3.secret_id]
        except KeyError as e:
            raise SecretsVaultError(msg=f"error obtaining S3 credentials: {e}") from e

        return hostname.rstrip(), access_id.rstrip(), secret_id.rstrip()

    @contextmanager
    def gpg_private_keyring(self) -> Generator[tuple[Path, str, str]]:
        with self.gpg_private_key_file() as pvt:
            self.log.debug("obtained gpg private key")
            pvt_key_file = pvt[0]
            passphrase = pvt[1]
            keyring_path = Path(tempfile.mkdtemp())
            keyring_path.chmod(0o700)

            self.log.debug(f"import gpg private key from '{pvt_key_file}'")
            cmd = ["gpg", "--import", "--batch", pvt_key_file.resolve().as_posix()]
            env = os.environ.copy()
            env["GNUPGHOME"] = keyring_path.resolve().as_posix()
            try:
                p = subprocess.run(cmd, capture_output=True, env=env)  # noqa: S603
                self.log.debug(f"stdout: {p.stdout}")
                self.log.debug(f"stderr: {p.stderr}")
            except Exception as e:
                msg = f"error importing gpg private key: {e}"
                self.log.exception(msg)
                raise SecretsVaultError(msg) from e

            if p.returncode != 0:
                logger.error(
                    f"error importing gpg private key from '{pvt_key_file}': {p.stderr}"
                )
                raise SecretsVaultError(
                    msg=f"error importing gpg private key: {p.stderr}"
                )

            self.log.debug(
                f"return keyring at '{keyring_path}', "
                + f"email '{self.secrets.gpg.extras.email}'"
            )
            yield keyring_path, passphrase, self.secrets.gpg.extras.email

            try:
                shutil.rmtree(keyring_path)
            except Exception as e:
                msg = f"error cleaning up keyring at '{keyring_path}': {e}"
                logger.exception(msg)
                raise SecretsVaultError(msg) from e

    @contextmanager
    def gpg_private_key_file(self) -> Generator[tuple[Path, str]]:
        # obtain private key from vault
        try:
            gpg_pvt_secret = self.vault.read_secret(self.secrets.gpg.private.vault_key)
        except VaultError as e:
            raise SecretsVaultError(
                msg=f"error obtaining GPG private key from vault: {e}"
            ) from e

        try:
            gpg_pvt_key = gpg_pvt_secret[self.secrets.gpg.private.key]
            gpg_pvt_passphrase = gpg_pvt_secret[self.secrets.gpg.private.passphrase]
        except KeyError as e:
            raise SecretsVaultError(
                msg=f"error obtaining GPG private key credentials: {e}"
            ) from e

        try:
            _, gpg_pvt_file = tempfile.mkstemp()
            gpg_pvt_path = Path(gpg_pvt_file)
            with gpg_pvt_path.open("w") as f:
                n = f.write(gpg_pvt_key)
        except Exception as e:
            msg = f"error writing gpg private key to file: {e}"
            self.log.exception(msg)
            raise SecretsVaultError(msg) from e

        yield gpg_pvt_path, gpg_pvt_passphrase.rstrip()

        try:
            with gpg_pvt_path.open("bw") as f:
                _ = f.write(random.randbytes(n))  # noqa: S311
            gpg_pvt_path.unlink()
        except Exception as e:
            msg = f"error cleaning up gpg private key file: {e}"
            self.log.exception(msg)
            raise SecretsVaultError(msg) from e

    @contextmanager
    def gpg_public_key_file(self) -> Generator[str]:
        # obtain public key from vault
        try:
            gpg_pub_secret = self.vault.read_secret(self.secrets.gpg.public.vault_key)
        except VaultError as e:
            raise SecretsVaultError(
                msg=f"error obtainin GPG public key from vault: {e}"
            ) from e

        try:
            gpg_pub_key = gpg_pub_secret[self.secrets.gpg.public.key]
        except KeyError as e:
            raise SecretsVaultError(msg=f"error obtaining GPG public key: {e}") from e

        _, gpg_pub_path = tempfile.mkstemp()
        with Path(gpg_pub_path).open("w") as f:
            n = f.write(gpg_pub_key)

        yield gpg_pub_path

        with Path(gpg_pub_path).open("bw") as f:
            _ = f.write(random.randbytes(n))  # noqa: S311
        os.unlink(gpg_pub_path)
