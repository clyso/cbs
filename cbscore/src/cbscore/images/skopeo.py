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

from cbscore.errors import CESError, UnknownRepositoryError
from cbscore.images import get_image_name
from cbscore.images import logger as parent_logger
from cbscore.images.errors import SkopeoError
from cbscore.images.signing import sign
from cbscore.utils import CmdArgs, Password, run_cmd
from cbscore.utils.secrets import SecretsVaultError, SecretsVaultMgr

logger = parent_logger.getChild("skopeo")


class SkopeoTagListResult(pydantic.BaseModel):
    repository: str = pydantic.Field(alias="Repository")
    tags: list[str] = pydantic.Field(alias="Tags")


def skopeo(args: CmdArgs) -> tuple[int, str, str]:
    cmd: CmdArgs = ["skopeo", *args]
    return run_cmd(cmd)


def skopeo_get_tags(img: str) -> SkopeoTagListResult:
    img_base = get_image_name(img)
    try:
        retcode, raw_out, err = skopeo(["list-tags", f"docker://{img_base}"])
    except CESError as e:
        logger.exception(f"error obtaining image tags for {img_base}")
        raise e  # noqa: TRY201

    if retcode != 0:
        m = re.match(r".*repository.*not found.*", err)
        if m is not None:
            raise UnknownRepositoryError(img_base)
        raise SkopeoError()

    try:
        return SkopeoTagListResult.model_validate_json(raw_out)
    except pydantic.ValidationError:
        logger.exception("unable to parse resulting images list")
        raise SkopeoError() from None


def skopeo_copy(src: str, dst: str, secrets: SecretsVaultMgr) -> None:
    logger.info(f"copy '{src}' to '{dst}'")

    try:
        _, user, passwd = secrets.harbor_creds()
    except SecretsVaultError as e:
        logger.exception("error obtaining harbor credentials")
        raise e  # noqa: TRY201

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
        logger.exception("error copying images")
        raise e  # noqa: TRY201

    if retcode != 0:
        logger.error(f"error copying images: {err}")
        raise SkopeoError()

    logger.info(f"copied '{src}' to '{dst}'")

    try:
        retcode, out, err = sign(dst, secrets)
    except SkopeoError as e:
        logger.exception(f"error signing image '{dst}'")
        raise e  # noqa: TRY201

    if retcode != 0:
        logger.error(f"error signing image '{dst}': {err}")
        raise SkopeoError()

    logger.info(f"signed image '{dst}': {out}")
