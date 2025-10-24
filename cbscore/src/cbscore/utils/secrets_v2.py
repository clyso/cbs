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
# pyright_foo: reportAny=false, reportUnknownArgumentType=false

import pydantic


class VaultSecret(pydantic.BaseModel):
    """Base Vault secret -- identified by a key to read."""

    vault_key: str = pydantic.Field(alias="vault-key")


class GitSSHSecretFields(pydantic.BaseModel):
    """SSH Git access requires an SSH key."""

    ssh_key: str = pydantic.Field(alias="ssh-key")


class GitHTTPSSecretFields(pydantic.BaseModel):
    """HTTPS Git access requires a username and a password."""

    username: str
    password: str


class GitSSHSecret(VaultSecret):
    """Git secret for SSH access."""

    fields: GitSSHSecretFields
    extras: dict[str, str] = pydantic.Field(default={})


class GitHTTPSSecret(VaultSecret):
    """Git secret for HTTPS access."""

    fields: GitHTTPSSecretFields


class S3Secret(VaultSecret):
    pass
