# cbsbuild - commands - config
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
import json
import sys
from pathlib import Path
from typing import cast

import click
import yaml

from cbscore.cmds import Ctx, pass_ctx
from cbscore.config import (
    ArtifactsConfig,
    ArtifactsS3Config,
    Config,
    ConfigError,
    DefaultSecretsConfig,
    PathsConfig,
    VaultAppRoleConfig,
    VaultConfig,
    VaultUserPassConfig,
)


def config_init_vault(cwd: Path, vault_addr: str | None) -> Path:
    default_vault_config_path = cwd / "cbs-build.vault.yaml"
    vault_config_path = cast(
        Path,
        click.prompt(
            "Vault config path",
            default=default_vault_config_path,
            type=click.Path(path_type=Path, exists=False),
        ),
    )
    if vault_config_path.exists() and not click.confirm(
        f"Vault config path '{vault_config_path}' already exists. Overwrite?"
    ):
        return vault_config_path

    if not vault_addr:
        vault_addr = cast(
            str,
            click.prompt(
                "Vault address",
                type=str,
            ),
        )

    vault_auth_user: VaultUserPassConfig | None = None
    vault_approle: VaultAppRoleConfig | None = None
    vault_token: str | None = None

    if click.confirm("Specify user/pass auth for vault?"):
        username = cast(str, click.prompt("Username", type=str))
        password = cast(str, click.prompt("Password", hide_input=True, type=str))
        vault_auth_user = VaultUserPassConfig(username=username, password=password)
    elif click.confirm("Specify AppRole auth for vault?"):
        role_id = cast(str, click.prompt("Role ID", type=str))
        secret_id = cast(str, click.prompt("Secret ID", type=str))
        vault_approle = VaultAppRoleConfig(role_id=role_id, secret_id=secret_id)
    else:
        token = cast(str | None, click.prompt("Vault token"))
        if not token:
            print("must provide a vault token")
            sys.exit(errno.EINVAL)
        vault_token = token

    vault_config = VaultConfig(
        vault_addr=vault_addr,
        auth_user=vault_auth_user,
        auth_approle=vault_approle,
        auth_token=vault_token,
    )

    try:
        vault_config.store(vault_config_path)
    except ConfigError as e:
        click.echo(
            f"error writing vault config to '{vault_config_path}': {e}", err=True
        )
        sys.exit(errno.EIO)
    return vault_config_path


def config_init_paths(cwd: Path) -> PathsConfig:
    components_paths: list[Path] = []
    default_components_path = cwd / "components"
    if default_components_path.exists() and click.confirm(
        f"Use '{default_components_path}' as components path?"
    ):
        components_paths.append(default_components_path)

    components_confirm_str = "Specify additional" if components_paths else "Specify"
    if click.confirm(f"{components_confirm_str} paths?"):
        while True:
            comp_path = Path(
                cast(str, click.prompt("Components path", type=str))
            ).resolve()
            if not comp_path.exists() or not comp_path.is_dir():
                print(
                    f"provided path '{comp_path}' does not exist or is not a directory"
                )
            else:
                components_paths.append(comp_path)

            if not click.confirm("Add another components path?"):
                break

    scratch_path = Path(cast(str, click.prompt("Scratch path", type=str))).resolve()
    scratch_containers_path = Path(
        cast(str, click.prompt("Scratch containers path", type=str))
    ).resolve()
    ccache_path: Path | None = None
    if click.confirm("Specify ccache path?"):
        ccache_path = Path(cast(str, click.prompt("ccache path", type=str))).resolve()

    return PathsConfig(
        components=components_paths,
        scratch=scratch_path,
        scratch_containers=scratch_containers_path,
        ccache=ccache_path,
    )


def config_init_artifacts() -> ArtifactsConfig | None:
    if not click.confirm("Configure S3 artifacts locations?"):
        return None

    artifact_bucket = cast(str, click.prompt("S3 artifacts bucket", type=str))
    releases_bucket = cast(str, click.prompt("S3 releases bucket", type=str))
    return ArtifactsConfig(
        s3=ArtifactsS3Config(
            s3_artifact_bucket=artifact_bucket, s3_releases_bucket=releases_bucket
        )
    )


def config_init_secrets_paths() -> list[Path]:
    if not click.confirm("Specify paths to secrets file(s)?"):
        return []

    secrets_paths: list[Path] = []
    secrets_file_path = Path(cast(str, click.prompt("Path to secrets file"))).resolve()
    secrets_paths.append(secrets_file_path)

    while True:
        if not click.confirm("Specify additional secrets files?"):
            break
        secrets_file_path = Path(
            cast(str, click.prompt("Path to secrets file"))
        ).resolve()
        secrets_paths.append(secrets_file_path)

    return secrets_paths


def config_init_secrets_config() -> DefaultSecretsConfig | None:
    if not click.confirm("Specify default secrets to use?"):
        return None

    storage_secret = cast(str, click.prompt("Storage secret", default="", type=str))
    gpg_signing_secret = cast(
        str, click.prompt("GPG signing secret", default="", type=str)
    )
    transit_signing_secret = cast(
        str,
        click.prompt("Transit signing secret", default="", type=str),
    )
    registry = cast(str, click.prompt("Registry", default="", type=str))
    return DefaultSecretsConfig(
        storage=storage_secret if storage_secret else None,
        gpg_signing=gpg_signing_secret if gpg_signing_secret else None,
        transit_signing=transit_signing_secret if transit_signing_secret else None,
        registry=registry if registry else None,
    )


def config_init(
    config_path: Path,
    cwd: Path,
    vault_addr: str | None,
) -> None:
    vault_config_path: Path | None = None
    if click.confirm("Configure vault authentication?"):
        vault_config_path = config_init_vault(cwd, vault_addr)

    config_paths = config_init_paths(cwd)
    artifacts = config_init_artifacts()
    secrets_paths = config_init_secrets_paths()
    secrets_config = config_init_secrets_config()

    config = Config(
        paths=config_paths,
        artifacts=artifacts,
        secrets_config=secrets_config,
        secrets=secrets_paths,
        vault=vault_config_path,
    )

    if config_path.suffix != ".yaml":
        new_config_path = config_path.with_suffix(".yaml")
        click.echo(
            f"config at '{config_path}' not YAML, use '{new_config_path}' instead."
        )
        config_path = new_config_path

    if config_path.exists() and not click.confirm("Config file exists, overwrite?"):
        click.echo(f"do not write config file to '{config_path}'", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo("config:\n")
    json_dict = json.loads(config.model_dump_json())  # pyright: ignore[reportAny]
    click.echo(yaml.safe_dump(json_dict, indent=2))

    if not click.confirm(f"Write config to '{config_path}'?"):
        click.echo("do not write config files")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        config.store(config_path)
    except ConfigError as e:
        click.echo(f"error writing config file: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"wrote config file to '{config_path}'")


@click.group("config", help="Config related operations.")
def cmd_config() -> None:
    pass


@cmd_config.command("init", help="Initialize the configuration file.")
@click.option(
    "--vault-addr",
    "vault_addr",
    type=str,
    required=False,
    prompt="Vault address",
)
@pass_ctx
def cmd_config_init(ctx: Ctx, vault_addr: str | None) -> None:
    assert ctx.config_path
    config_path = ctx.config_path

    cwd = Path.cwd()

    config_init(config_path, cwd, vault_addr)
