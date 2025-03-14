# CES library - images skopeo utilities
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

import re

import pydantic
from ceslib.errors import CESError, UnknownRepositoryError
from ceslib.images import get_image_name
from ceslib.images import log as parent_logger
from ceslib.images.errors import SkopeoError
from ceslib.images.signing import sign
from ceslib.utils import CmdArgs, Password, run_cmd
from ceslib.utils.secrets import SecretsVaultError, SecretsVaultMgr

log = parent_logger.getChild("skopeo")


class SkopeoTagListResult(pydantic.BaseModel):
    repository: str = pydantic.Field(alias="Repository")
    tags: list[str] = pydantic.Field(alias="Tags")


def skopeo(args: CmdArgs) -> tuple[int, str, str]:
    cmd: CmdArgs = ["skopeo"] + args
    return run_cmd(cmd)


def skopeo_get_tags(img: str) -> SkopeoTagListResult:
    img_base = get_image_name(img)
    try:
        retcode, raw_out, err = skopeo(["list-tags", f"docker://{img_base}"])
    except CESError as e:
        log.error(f"error obtaining image tags for {img_base}")
        raise e

    if retcode != 0:
        m = re.match(r".*repository.*not found.*", err)
        if m is not None:
            raise UnknownRepositoryError(img_base)
        raise SkopeoError()

    try:
        return SkopeoTagListResult.model_validate_json(raw_out)
    except pydantic.ValidationError as e:
        log.error(f"unable to parse resulting images list: {e}")
        raise SkopeoError()


def skopeo_copy(src: str, dst: str, secrets: SecretsVaultMgr) -> None:
    log.info(f"copy '{src}' to '{dst}'")

    try:
        _, user, passwd = secrets.harbor_creds()
    except SecretsVaultError as e:
        log.error(f"error obtaining harbor credentials: {e}")
        raise e

    try:
        retcode, _, err = skopeo(
            [
                "copy",
                "--dest-creds",
                Password(f"{user}:{passwd}"),
                f"docker://{src}",
                f"docker://{dst}",
            ]
        )
    except SkopeoError as e:
        log.error(f"error copying images: {e}")
        raise e

    if retcode != 0:
        log.error(f"error copying images: {err}")
        raise SkopeoError()

    log.info(f"copied '{src}' to '{dst}'")

    try:
        retcode, out, err = sign(dst, secrets)
    except SkopeoError as e:
        log.error(f"error signing image '{dst}': {e}")
        raise e

    if retcode != 0:
        log.error(f"error signing image '{dst}': {err}")
        raise SkopeoError()

    log.info(f"signed image '{dst}': {out}")
