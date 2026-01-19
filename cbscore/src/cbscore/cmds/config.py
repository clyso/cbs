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
    Config,
    ConfigError,
    PathsConfig,
    RegistryStorageConfig,
    S3LocationConfig,
    S3StorageConfig,
    SigningConfig,
    StorageConfig,
    VaultAppRoleConfig,
    VaultConfig,
    VaultUserPassConfig,
)


def config_init_vault(cwd: Path, vault_config_path: Path | None) -> Path | None:
    """Initialize vault config interactively."""
    if vault_config_path and vault_config_path.exists():
        return vault_config_path

    if not click.confirm("Configure vault authentication?"):
        click.echo("skipping vault configuration")
        return None

    if not vault_config_path:
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


def config_init_paths(
    cwd: Path,
    *,
    components_paths: list[Path] | None = None,
    scratch_path: Path | None = None,
    containers_scratch_path: Path | None = None,
    ccache_path: Path | None = None,
) -> PathsConfig:
    """Initialize paths config interactively."""
    components_paths = components_paths if components_paths else []

    if not components_paths:
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
                        f"provided path '{comp_path}' does not exist "
                        + "or is not a directory"
                    )
                else:
                    components_paths.append(comp_path)

                if not click.confirm("Add another components path?"):
                    break

    if not scratch_path:
        scratch_path = Path(cast(str, click.prompt("Scratch path", type=str))).resolve()

    if not containers_scratch_path:
        containers_scratch_path = Path(
            cast(str, click.prompt("Scratch containers path", type=str))
        ).resolve()

    if not ccache_path and click.confirm("Specify ccache path?"):
        ccache_path = Path(cast(str, click.prompt("ccache path", type=str))).resolve()

    return PathsConfig(
        components=components_paths,
        scratch=scratch_path,
        scratch_containers=containers_scratch_path,
        ccache=ccache_path,
    )


def config_init_storage() -> StorageConfig | None:
    """Initialize storage config interactively."""
    if not click.confirm("Configure storage?"):
        click.echo("skipping storage configuration")
        click.echo("this should be manually configured later if storage is used")
        return None

    s3_storage_config: S3StorageConfig | None = None
    if click.confirm("Configure S3 storage for artifact upload?"):
        s3_url = cast(str, click.prompt("S3 storage URL", type=str))
        artifact_bucket = cast(str, click.prompt("S3 artifacts bucket", type=str))
        artifact_loc = cast(str, click.prompt("S3 artifacts location", type=str))
        releases_bucket = cast(str, click.prompt("S3 releases bucket", type=str))
        releases_loc = cast(str, click.prompt("S3 releases location", type=str))
        s3_storage_config = S3StorageConfig(
            url=s3_url,
            artifacts=S3LocationConfig(
                bucket=artifact_bucket,
                loc=artifact_loc,
            ),
            releases=S3LocationConfig(
                bucket=releases_bucket,
                loc=releases_loc,
            ),
        )

    registry_storage_config: RegistryStorageConfig | None = None
    # FIXME: container registry location config is currently ignored, given the image
    # is always uploaded to the registry specified in the version descriptor. Yet, we
    # need the config option otherwise checks in the builder will refuse to build.
    if click.confirm("Configure registry storage for container image upload?"):
        registry_url = cast(str, click.prompt("Registry storage URL", type=str))
        registry_storage_config = RegistryStorageConfig(url=registry_url)

    return StorageConfig(
        s3=s3_storage_config,
        registry=registry_storage_config,
    )


def config_init_signing() -> SigningConfig | None:
    """Initialize signing config interactively."""
    if not click.confirm("Configure artifact signing?"):
        click.echo("skipping signing configuration")
        click.echo("this should be manually configured later if signing is used")
        return None

    gpg_secret: str | None = None
    if click.confirm("Specify package GPG signing secret name?"):
        gpg_secret = cast(str, click.prompt("GPG signing secret name", type=str))

    transit_secret: str | None = None
    if click.confirm("Specify container image transit signing secret name?"):
        transit_secret = cast(
            str, click.prompt("Transit signing secret name", type=str)
        )

    if gpg_secret is None and transit_secret is None:
        click.echo("no signing methods specified, skipping signing configuration")
        return None

    return SigningConfig(gpg=gpg_secret, transit=transit_secret)


def config_init_secrets_paths(secrets_files_paths: list[Path] | None) -> list[Path]:
    """Return a list of paths for files containing secrets."""
    if secrets_files_paths:
        return secrets_files_paths

    if not click.confirm("Specify paths to secrets file(s)?"):
        click.echo("skipping secrets files configuration")
        click.echo("these should be manually configured later")
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


def config_init(
    config_path: Path,
    cwd: Path,
    *,
    components_paths: list[Path] | None = None,
    scratch_path: Path | None = None,
    containers_scratch_path: Path | None = None,
    ccache_path: Path | None = None,
    vault_config_path: Path | None = None,
    secrets_files_paths: list[Path] | None = None,
) -> None:
    """Initialize config interactively and store to config_path."""
    config_paths = config_init_paths(
        cwd,
        components_paths=components_paths,
        scratch_path=scratch_path,
        containers_scratch_path=containers_scratch_path,
        ccache_path=ccache_path,
    )
    storage_config = config_init_storage()
    signing_config = config_init_signing()

    secrets_paths = config_init_secrets_paths(secrets_files_paths)

    config = Config(
        paths=config_paths,
        storage=storage_config,
        signing=signing_config,
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

    config_path.parent.mkdir(exist_ok=True, parents=True)
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
    "--components",
    "components_paths",
    type=click.Path(
        path_type=Path,
        exists=False,
        file_okay=False,
        dir_okay=True,
    ),
    required=False,
    multiple=True,
    help="Specify components path.",
)
@click.option(
    "--scratch",
    "scratch_path",
    type=click.Path(
        path_type=Path,
        exists=False,
        file_okay=False,
        dir_okay=True,
    ),
    required=False,
    help="Specify scratch path.",
)
@click.option(
    "--containers-scratch",
    "containers_scratch_path",
    type=click.Path(
        path_type=Path,
        exists=False,
        file_okay=False,
        dir_okay=True,
    ),
    required=False,
    help="Specify containers scratch path.",
)
@click.option(
    "--ccache",
    "ccache_path",
    type=click.Path(
        path_type=Path,
        exists=False,
        file_okay=False,
        dir_okay=True,
    ),
    required=False,
    help="Specify ccache path.",
)
@click.option(
    "--vault",
    "vault_config_path",
    type=click.Path(
        path_type=Path,
        exists=False,
        file_okay=True,
        dir_okay=False,
    ),
    required=False,
    help="Specify vault config path.",
)
@click.option(
    "--secrets",
    "secrets_files_paths",
    type=click.Path(
        path_type=Path,
        exists=False,
        file_okay=True,
        dir_okay=False,
    ),
    multiple=True,
    required=False,
    help="Specify secrets files paths.",
)
@click.option(
    "--for-systemd-install",
    "paths_for_systemd_install",
    is_flag=True,
    default=False,
    required=False,
    help="Initialize paths for a systemd install.",
)
@click.option(
    "--systemd-deployment",
    "systemd_deployment",
    type=str,
    required=False,
    default="default",
    show_default=True,
    help="Systemd deployment name.",
)
@click.option(
    "--for-containerized-run",
    "paths_for_containerized_run",
    is_flag=True,
    default=False,
    required=False,
    help="Initialize paths for a containerized run.",
)
@pass_ctx
def cmd_config_init(
    ctx: Ctx,
    components_paths: tuple[Path, ...] | None,
    scratch_path: Path | None,
    containers_scratch_path: Path | None,
    ccache_path: Path | None,
    vault_config_path: Path | None,
    secrets_files_paths: tuple[Path, ...] | None,
    paths_for_systemd_install: bool,
    systemd_deployment: str | None,
    paths_for_containerized_run: bool,
) -> None:
    assert ctx.config_path
    config_path = ctx.config_path

    cwd = Path.cwd()

    if paths_for_systemd_install or paths_for_containerized_run:
        components_paths = [Path("/cbs/components")]
        scratch_path = Path("/cbs/scratch")
        containers_scratch_path = Path("/var/lib/containers")
        ccache_path = Path("/cbs/ccache")
        secrets_files_paths = [Path("/cbs/config/secrets.yaml")]
        vault_config_path = Path("/cbs/config/vault.yaml")

    if paths_for_systemd_install:
        config_dir_path = Path.home() / f".config/cbsd/{systemd_deployment}/worker"
        config_path = config_dir_path / "cbscore.config.yaml"

    config_init(
        config_path,
        cwd,
        components_paths=list(components_paths) if components_paths else None,
        scratch_path=scratch_path,
        containers_scratch_path=containers_scratch_path,
        ccache_path=ccache_path,
        vault_config_path=vault_config_path,
        secrets_files_paths=list(secrets_files_paths) if secrets_files_paths else None,
    )


@cmd_config.command("init-vault", help="Initialize the vault configuration file.")
@click.option(
    "--vault",
    "vault_config_path",
    type=click.Path(
        path_type=Path,
        exists=False,
        file_okay=True,
        dir_okay=False,
    ),
    required=False,
    help="Specify vault config path.",
)
def cmd_config_init_vault(vault_config_path: Path | None) -> None:
    cwd = Path.cwd()
    path = config_init_vault(cwd, vault_config_path)
    if not path:
        click.echo("vault configuration not initialized")
    else:
        click.echo(f"vault configuration available at '{path}'")
