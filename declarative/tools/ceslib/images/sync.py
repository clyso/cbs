# CES library - images sync
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

from ceslib.errors import CESError, UnknownRepositoryError
from ceslib.images import get_image_tag, skopeo
from ceslib.images import log as parent_logger
from ceslib.images.auth import AuthAndSignInfo
from ceslib.images.errors import MissingTagError

log = parent_logger.getChild("sync")


def sync_image(
    src: str,
    dst: str,
    auth_info: AuthAndSignInfo,
    *,
    force: bool = False,
    dry_run: bool = False,
) -> None:
    log.debug(f"sync image from '{src}' to '{dst}'")
    src_tag = get_image_tag(src)
    dst_tag = get_image_tag(dst)

    if src_tag is None:
        log.error(f"missing tag for source image '{src}'")
        raise MissingTagError(for_what=src)
    if dst_tag is None:
        log.debug(f"missing tag for dest image '{dst}', assume '{src_tag}'")
        dst_tag = src_tag

    try:
        res_src = skopeo.skopeo_get_tags(src)
    except UnknownRepositoryError as e:
        log.error(f"unable to obtain information for source repository: {e}")
        raise e
    except Exception as e:
        log.error(f"unknown error: {e}")
        raise e

    missing_dst_repo = False
    res_dst: skopeo.SkopeoTagListResult | None = None
    try:
        res_dst = skopeo.skopeo_get_tags(dst)
    except UnknownRepositoryError:
        missing_dst_repo = True
    except Exception as e:
        log.error(f"unknown error: {e}")
        raise e

    if src_tag not in res_src.tags:
        log.error(f"error: missing source tag '{src_tag}' for '{src}'")
        raise MissingTagError(tag=src_tag, for_what=src)

    if not missing_dst_repo and not force:
        assert res_dst is not None
        if dst_tag in res_dst.tags:
            log.debug(f"nothing to do for tag '{dst_tag}' for '{dst}'")
            return

    log.debug(f"copying '{src}' to '{dst}")
    try:
        if not dry_run:
            log.debug(f"copy '{src}' to '{dst}'")
            skopeo.skopeo_copy(src, dst, auth_info)
        else:
            log.debug("not copying, dry run specified")
    except CESError as e:
        log.error(f"error copying image '{src}' to '{dst}': {e}")
        raise e
    except Exception as e:
        log.error(f"unknown error: {e}")
        raise e

    log.debug(f"copied image from '{src}' to '{dst}'")
