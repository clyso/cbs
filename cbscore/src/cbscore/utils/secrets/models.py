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

from __future__ import annotations

import sys
from pathlib import Path
from typing import Annotated, Any, ClassVar, Literal

import pydantic
import yaml

from cbscore.utils.secrets import SecretsError
from cbscore.utils.secrets import logger as parent_logger

logger = parent_logger.getChild("models")


class VaultSecret(pydantic.BaseModel):
    """Base Vault secret -- identified by a key to read."""

    creds: Literal["vault"] = "vault"
    key: str


class PlainSecret(pydantic.BaseModel):
    """Base plain secret."""

    creds: Literal["plain"] = "plain"


class GitSSHSecret(PlainSecret):
    """Git SSH secret stored in plain text."""

    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    ssh_key: Annotated[str, pydantic.Field(alias="ssh-key")]
    username: str


class GitTokenSecret(PlainSecret):
    """Git token secret stored in plain text."""

    token: str
    username: str


class GitHTTPSSecret(PlainSecret):
    """Git HTTPS secret stored in plain text."""

    username: str
    password: str


class GitVaultSSHSecret(VaultSecret):
    """Git secret stored in Vault."""

    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    ssh_key: Annotated[str, pydantic.Field(alias="ssh-key")]
    username: str


class GitVaultHTTPSSecret(VaultSecret):
    """Git HTTPS secret stored in Vault."""

    username: str
    password: str


def git_secret_discriminator(
    secret: Any,  # pyright: ignore[reportExplicitAny, reportAny]
) -> str | None:
    if not isinstance(secret, dict):
        if isinstance(secret, GitSSHSecret):
            return "plain-ssh"
        elif isinstance(secret, GitTokenSecret):
            return "plain-token"
        elif isinstance(secret, GitHTTPSSecret):
            return "plain-https"
        elif isinstance(secret, GitVaultSSHSecret):
            return "vault-ssh"
        elif isinstance(secret, GitVaultHTTPSSecret):
            return "vault-https"
        else:
            return None

    if "creds" not in secret:
        raise ValueError("missing 'creds' field in vault secret")

    if secret["creds"] == "vault":
        if "ssh-key" in secret:
            return "vault-ssh"
        elif "username" in secret and "password" in secret:
            return "vault-https"
        else:
            return None
    elif secret["creds"] == "plain":
        if "ssh-key" in secret:
            return "plain-ssh"
        elif "token" in secret:
            return "plain-token"
        elif "username" in secret and "password" in secret:
            return "plain-https"
        else:
            return None

    return None


GitSecret = Annotated[
    Annotated[GitSSHSecret, pydantic.Tag("plain-ssh")]
    | Annotated[GitTokenSecret, pydantic.Tag("plain-token")]
    | Annotated[GitHTTPSSecret, pydantic.Tag("plain-https")]
    | Annotated[GitVaultSSHSecret, pydantic.Tag("vault-ssh")]
    | Annotated[GitVaultHTTPSSecret, pydantic.Tag("vault-https")],
    pydantic.Discriminator(git_secret_discriminator),
]


class StorageS3Secret(pydantic.BaseModel):
    """Base S3 storage secret."""

    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    type: Literal["s3"] = "s3"


class StoragePlainS3Secret(PlainSecret, StorageS3Secret):
    """S3 Credentials stored in plain text."""

    access_id: Annotated[str, pydantic.Field(alias="access-id")]
    secret_id: Annotated[str, pydantic.Field(alias="secret-id")]


class StorageVaultS3Secret(VaultSecret, StorageS3Secret):
    """S3 Credentials stored in Vault."""

    access_id: Annotated[str, pydantic.Field(alias="access-id")]
    secret_id: Annotated[str, pydantic.Field(alias="secret-id")]


def storage_secret_discriminator(
    secret: Any,  # pyright: ignore[reportExplicitAny, reportAny]
) -> str | None:
    if not isinstance(secret, dict):
        if isinstance(secret, StoragePlainS3Secret):
            return "plain-s3"
        elif isinstance(secret, StorageVaultS3Secret):
            return "vault-s3"
        else:
            return None

    if "creds" not in secret:
        raise ValueError("missing 'creds' field in storage secret")
    elif "type" not in secret:
        return None

    if secret["creds"] == "vault" and secret["type"] == "s3":
        return "vault-s3"
    elif secret["creds"] == "plain" and secret["type"] == "s3":
        return "plain-s3"

    return None


StorageSecret = Annotated[
    Annotated[StoragePlainS3Secret, pydantic.Tag("plain-s3")]
    | Annotated[StorageVaultS3Secret, pydantic.Tag("vault-s3")],
    pydantic.Discriminator(storage_secret_discriminator),
]


class GPGPlainSecret(PlainSecret):
    """Plain GPG signing secret."""

    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    type: Literal["gpg"] = "gpg"

    private_key: Annotated[str, pydantic.Field(alias="private-key")]
    public_key: Annotated[str | None, pydantic.Field(alias="public-key", default=None)]
    passphrase: str | None = pydantic.Field(default=None)
    email: str


class GPGVaultSingleSecret(VaultSecret):
    """GPG signing secret stored in Vault (single key)."""

    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    type: Literal["gpg-single-key"] = "gpg-single-key"

    private_key: Annotated[str, pydantic.Field(alias="private-key")]
    public_key: Annotated[str | None, pydantic.Field(alias="public-key", default=None)]
    passphrase: str | None = pydantic.Field(default=None)
    email: str


class GPGVaultPrivateKeySecret(VaultSecret):
    """GPG signing secret stored in Vault (private key)."""

    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    type: Literal["gpg-pvt-key"] = "gpg-pvt-key"

    private_key: Annotated[str, pydantic.Field(alias="private-key")]
    passphrase: str | None = pydantic.Field(default=None)
    email: str


class GPGVaultPublicKeySecret(VaultSecret):
    """GPG signing secret stored in Vault (public key)."""

    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )

    type: Literal["gpg-pub-key"] = "gpg-pub-key"

    public_key: Annotated[str, pydantic.Field(alias="public-key")]
    email: str


class VaultTransitSecret(VaultSecret):
    """Vault Transit signing secret."""

    type: Literal["transit"] = "transit"

    mount: str


def signing_secret_discriminator(
    secret: Any,  # pyright: ignore[reportExplicitAny, reportAny]
) -> str | None:
    if not isinstance(secret, dict):
        if isinstance(secret, GPGPlainSecret):
            return "plain-gpg"
        elif isinstance(secret, GPGVaultSingleSecret):
            return "vault-gpg-single-key"
        elif isinstance(secret, GPGVaultPrivateKeySecret):
            return "vault-gpg-pvt-key"
        elif isinstance(secret, GPGVaultPublicKeySecret):
            return "vault-gpg-pub-key"
        elif isinstance(secret, VaultTransitSecret):
            return "vault-transit"
        else:
            return None

    if "creds" not in secret:
        raise ValueError("missing 'creds' field in signing secret")
    elif "type" not in secret:
        return None

    if secret["creds"] == "plain" and secret["type"] == "gpg":
        return "plain-gpg"
    elif secret["creds"] == "vault" and secret["type"] == "gpg-single-key":
        return "vault-gpg-single"
    elif secret["creds"] == "vault" and secret["type"] == "gpg-pvt-key":
        return "vault-gpg-pvt-key"
    elif secret["creds"] == "vault" and secret["type"] == "gpg-pub-key":
        return "vault-gpg-pub-key"
    elif secret["creds"] == "vault" and secret["type"] == "transit":
        return "vault-transit"

    return None


SigningSecret = Annotated[
    Annotated[GPGPlainSecret, pydantic.Tag("plain-gpg")]
    | Annotated[GPGVaultSingleSecret, pydantic.Tag("vault-gpg-single")]
    | Annotated[GPGVaultPrivateKeySecret, pydantic.Tag("vault-gpg-pvt-key")]
    | Annotated[GPGVaultPublicKeySecret, pydantic.Tag("vault-gpg-pub-key")]
    | Annotated[VaultTransitSecret, pydantic.Tag("vault-transit")],
    pydantic.Discriminator(signing_secret_discriminator),
]


class RegistryPlainSecret(PlainSecret):
    """Registry secret stored in plain text."""

    username: str
    password: str
    address: str


class RegistryVaultSecret(VaultSecret):
    """Registry secret stored in Vault."""

    username: str
    password: str
    address: str


def registry_secret_discriminator(
    secret: Any,  # pyright: ignore[reportExplicitAny, reportAny]
) -> str | None:
    if not isinstance(secret, dict):
        if isinstance(secret, RegistryPlainSecret):
            return "plain-registry"
        elif isinstance(secret, RegistryVaultSecret):
            return "vault-registry"
        else:
            return None

    if "creds" not in secret:
        raise ValueError("missing 'creds' field in registry secret")

    return (
        "plain-registry"
        if secret["creds"] == "plain"
        else ("vault-registry" if secret["creds"] == "vault" else None)
    )


RegistrySecret = Annotated[
    Annotated[RegistryPlainSecret, pydantic.Tag("plain-registry")]
    | Annotated[RegistryVaultSecret, pydantic.Tag("vault-registry")],
    pydantic.Discriminator(registry_secret_discriminator),
]


class Secrets(pydantic.BaseModel):
    """Secrets container."""

    git: dict[str, GitSecret] = pydantic.Field(default={})
    storage: dict[str, StorageSecret] = pydantic.Field(default={})
    sign: dict[str, SigningSecret] = pydantic.Field(default={})
    registry: dict[str, RegistrySecret] = pydantic.Field(default={})

    @classmethod
    def load(cls, path: Path) -> Secrets:
        if not path.exists() or not path.is_file():
            raise SecretsError(f"secrets file '{path}' does not exist or is not a file")

        try:
            raw_data = path.read_text()
            if path.suffix.lower() == ".yaml":
                secrets = Secrets.model_validate(yaml.safe_load(raw_data))
            else:
                secrets = Secrets.model_validate_json(raw_data)

        except (yaml.YAMLError, pydantic.ValidationError) as e:
            msg = f"error loading secrets at '{path}': {e}"
            logger.error(msg)
            raise SecretsError(msg) from e
        except Exception as e:
            msg = f"unexpected error loading secrets at '{path}': {e}"
            logger.error(msg)
            raise SecretsError(msg) from e

        return secrets

    def store(self, path: Path) -> None:
        """Store secrets to a specified path in YAML format."""
        try:
            raw_data = yaml.safe_dump(self.model_dump(), indent=2)
            _ = path.write_text(raw_data)
        except Exception as e:
            msg = f"error storing secrets to '{path}': {e}"
            logger.error(msg)
            raise SecretsError(msg) from e

    def merge(self, other: Secrets) -> None:
        """Merge another secrets object into this one."""
        self.git.update(other.git)
        self.storage.update(other.storage)
        self.sign.update(other.sign)
        self.registry.update(other.registry)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <secrets-path> [out]")
        sys.exit(1)

    try:
        secrets = Secrets.load(Path(sys.argv[1]))
        print(f"loaded secrets:\n{secrets}")
    except pydantic.ValidationError as e:
        print(f"Failed to load secrets: {e}")
        sys.exit(1)

    if len(sys.argv) >= 3:
        out_path = Path(sys.argv[2])
        try:
            secrets.store(out_path)
            print(f"stored secrets to '{out_path}'")
        except SecretsError as e:
            print(f"Failed to store secrets: {e}")
            sys.exit(1)
