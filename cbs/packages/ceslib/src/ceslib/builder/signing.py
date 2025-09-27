# CES library - CES builder rpm signing
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


import asyncio
import datetime
from datetime import datetime as dt
from pathlib import Path

from ceslib.builder import BuilderError
from ceslib.builder import logger as parent_logger
from ceslib.builder.rpmbuild import ComponentBuild
from ceslib.utils import CmdArgs, CommandError, async_run_cmd
from ceslib.utils.secrets import SecretsVaultMgr

logger = parent_logger.getChild("signing")


async def _sign_rpm(rpm_path: Path, keyring: Path, passphrase: str, email: str) -> None:
    logger.debug(f"sign rpm '{rpm_path}'")

    cmd: CmdArgs = [
        "rpm",
        "--addsign",
        "--define",
        f"_gpg_path {keyring.as_posix()}",
        "--define",
        f"_gpg_name {email}",
        "--define",
        f"_gpg_sign_cmd_extra_args --pinentry-mode loopback --passphrase {passphrase}",
        rpm_path.as_posix(),
    ]

    try:
        rc, _, stderr = await async_run_cmd(cmd)
    except CommandError as e:
        msg = f"error signing rpm '{rpm_path}': {e}"
        logger.exception(msg)
        raise BuilderError(msg) from e
    except Exception as e:
        msg = f"unknown error signing rpm '{rpm_path}': {e}"
        logger.exception(msg)
        raise BuilderError(msg) from e

    if rc != 0:
        msg = f"unable to sign rpm '{rpm_path}': {stderr}"
        logger.error(msg)
        raise BuilderError(msg)

    logger.debug(f"signed {rpm_path}")
    pass


async def _sign_component_rpms(
    path: Path, keyring: Path, passphrase: str, email: str
) -> tuple[int, int]:
    logger.info(f"sign component RPMs at '{path}'")

    rpms_to_sign: list[Path] = []

    start = dt.now(datetime.UTC)

    for parent, _, files in path.walk():
        for f in files:
            if f.endswith(".rpm"):
                rpms_to_sign.append(parent.joinpath(f))

    # NOTE: this can be parallelized, but we'll leave that as a future exercise.
    for rpm_path in rpms_to_sign:
        try:
            await _sign_rpm(rpm_path, keyring, passphrase, email)
        except BuilderError as e:
            msg = f"unable to sign rpms in '{path}': {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e
        except Exception as e:
            msg = f"unknown error signing rpms in '{path}': {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

    time_spent = (dt.now(tz=datetime.UTC) - start).seconds
    return time_spent, len(rpms_to_sign)


async def sign_rpms(
    secrets: SecretsVaultMgr, components_rpms_paths: dict[str, ComponentBuild]
) -> None:
    logger.info(f"sign rpms for {components_rpms_paths.keys()}")
    try:
        with secrets.gpg_private_keyring() as keyring:
            keyring_path = keyring[0]
            passphrase = keyring[1]
            email = keyring[2]

            logger.debug(f"sign with keyring at '{keyring_path}', email '{email}'")

            async with asyncio.TaskGroup() as tg:
                tasks = {
                    name: tg.create_task(
                        _sign_component_rpms(
                            p.rpms_path, keyring_path, passphrase, email
                        )
                    )
                    for name, p in components_rpms_paths.items()
                }
    except ExceptionGroup as e:
        excs = e.subgroup(BuilderError)
        if excs is not None:
            logger.error("error signing components RPMs:")
            for exc in excs.exceptions:
                logger.error(f"- {exc}")
        else:
            logger.error(f"unexpected error signing component RPMs: {e}")

        raise BuilderError(msg=f"error signing component RPMs: {e}") from e

    except Exception as e:
        msg = f"unexpected error signing RPMs: {e}"
        logger.exception(msg)
        raise BuilderError(msg) from e

    for name, task in tasks.items():
        time_spent, num_signs = task.result()
        logger.info(
            f"signed component '{name}' in {time_spent} seconds ({num_signs} signed)"
        )

    pass
