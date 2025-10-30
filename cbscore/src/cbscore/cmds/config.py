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
import sys
from pathlib import Path
from typing import cast

import click

from cbscore.cmds import Ctx, pass_ctx
from cbscore.config import Config, VaultAppRoleConfig, VaultConfig, VaultUserPassConfig


def config_init(
    config_path: Path,
    vault_config_path: Path,
    cwd: Path,
    vault_addr: str,
    vault_transit: str,
) -> None:
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
        vault_transit=vault_transit,
        auth_user=vault_auth_user,
        auth_approle=vault_approle,
        auth_token=vault_token,
    )

    components_paths: list[Path] = []
    default_components_path = cwd / "components"
    if default_components_path.exists() and click.confirm(
        f"Use '{default_components_path}' as components path?"
    ):
        components_paths.append(default_components_path)

    if click.confirm("Specify components paths?"):
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

    default_secrets_paths = [
        cwd / "cbs" / "secrets.json",
        cwd / "_local" / "secrets.json",
        cwd / "secrets.json",
    ]
    secrets_path: Path | None = None
    for entry in default_secrets_paths:
        if entry.exists() and entry.is_file():
            secrets_path = entry
            break

    secrets_prompt_str = ""
    if not secrets_path:
        print("unable to find default 'secrets.json' file")
    else:
        print(f"using '{secrets_path}' as default 'secrets.json' file")
        secrets_prompt_str = "a different "

    if click.confirm(f"Specify {secrets_prompt_str}path to 'secrets.json'?"):
        secrets_path = Path(cast(str, click.prompt("Path to 'secrets.json'"))).resolve()

    if not secrets_path:
        print("path to 'secrets.json' not provided, abort")
        sys.exit(errno.EINVAL)

    scratch_path = Path(cast(str, click.prompt("Scratch path", type=str))).resolve()
    scratch_containers_path = Path(
        cast(str, click.prompt("Scratch containers path", type=str))
    ).resolve()
    ccache_path: Path | None = None
    if click.confirm("Specify ccache path?"):
        ccache_path = Path(cast(str, click.prompt("ccache path", type=str))).resolve()

    config = Config(
        components_path=components_paths,
        secrets_path=secrets_path,
        scratch_path=scratch_path,
        scratch_containers_path=scratch_containers_path,
        ccache_path=ccache_path,
    )

    if config_path.exists() and not click.confirm("Config file exists, overwrite?"):
        print(f"do not write config file to '{config_path}'")
        sys.exit(errno.ENOTRECOVERABLE)

    if vault_config_path.exists() and not click.confirm(
        "Vault config exists, overwrite?"
    ):
        print(f"do not write vault config to '{vault_config_path}'")
        sys.exit(errno.ENOTRECOVERABLE)

    print(f"""
==== vault ====
{vault_config.model_dump_json(indent=2)}

==== config ====
{config.model_dump_json(indent=2)}

""")

    if not click.confirm("Write config?"):
        print("do not write config files")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        _ = config_path.write_text(config.model_dump_json(indent=2))
        _ = vault_config_path.write_text(vault_config.model_dump_json(indent=2))
    except Exception as e:
        print(f"error writing config file: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    print(f"""
wrote config files

      config: {config_path}
vault config: {vault_config_path}
""")


@click.group("config", help="Config related operations.")
def cmd_config() -> None:
    pass


@cmd_config.command("init", help="Initialize the configuration file.")
@click.option(
    "--vault-addr",
    "vault_addr",
    type=str,
    required=True,
    prompt="Vault address",
)
@click.option(
    "--vault-transit",
    "vault_transit",
    type=str,
    required=True,
    prompt="Vault transit",
)
@pass_ctx
def cmd_config_init(ctx: Ctx, vault_addr: str, vault_transit: str) -> None:
    assert ctx.config_path
    assert ctx.vault_config_path
    config_path = ctx.config_path
    vault_config_path = ctx.vault_config_path

    cwd = Path.cwd()

    config_init(config_path, vault_config_path, cwd, vault_addr, vault_transit)
