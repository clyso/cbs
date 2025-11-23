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
from cbscore.images.errors import ImageNotFoundError, SkopeoError
from cbscore.images.signing import sign
from cbscore.utils import CmdArgs, Password, run_cmd
from cbscore.utils.containers import get_container_image_base_uri
from cbscore.utils.secrets import SecretsMgrError
from cbscore.utils.secrets.mgr import SecretsMgr

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


def skopeo_copy(
    src: str,
    dst: str,
    dst_registry: str,
    secrets: SecretsMgr,
    transit: str,
) -> None:
    logger.info(f"copy '{src}' to '{dst}'")

    try:
        _, user, passwd = secrets.registry_creds(dst_registry)
    except SecretsMgrError as e:
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
        retcode, out, err = sign(dst_registry, dst, secrets, transit)
    except SkopeoError as e:
        logger.exception(f"error signing image '{dst}'")
        raise e  # noqa: TRY201

    if retcode != 0:
        logger.error(f"error signing image '{dst}': {err}")
        raise SkopeoError()

    logger.info(f"signed image '{dst}': {out}")


def skopeo_inspect(img: str, secrets: SecretsMgr) -> str:
    logger.debug(f"inspect image '{img}'")

    try:
        uri = get_container_image_base_uri(img)
    except ValueError as e:
        msg = f"error obtaining base image URI: {e}"
        logger.error(msg)
        raise SkopeoError(msg) from e

    try:
        _, user, passwd = secrets.registry_creds(uri)
    except SecretsMgrError as e:
        logger.error(f"error obtaining registry credentials: {e}")
        raise e from None

    try:
        retcode, raw_out, err = skopeo(
            [
                "inspect",
                "--creds",
                Password(f"{user}:{passwd}"),
                f"docker://{img}",
            ]
        )
    except SkopeoError as e:
        logger.exception(f"error inspecting image '{img}'")
        raise e  # noqa: TRY201

    if retcode != 0:
        msg = f"error inspecting image '{img}': {err}"
        logger.error(msg)
        if re.match(r".*not\s+found.*", err):
            raise ImageNotFoundError(img) from None
        raise SkopeoError(msg) from None

    return raw_out


def skopeo_image_exists(img: str, secrets: SecretsMgr) -> bool:
    logger.debug(f"check if image '{img}' exists")

    try:
        _ = skopeo_inspect(img, secrets)
    except ImageNotFoundError:
        logger.debug(f"image '{img}' does not exist")
        return False
    except SkopeoError as e:
        logger.exception(f"error checking if image '{img}' exists")
        raise e from None

    logger.debug(f"image '{img}' exists")
    return True
