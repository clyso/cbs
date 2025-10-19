# CES library - images
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

from cbscore.logger import logger as root_logger

logger = root_logger.getChild("images")


def get_image_name(img: str) -> str:
    idx = img.find(":")
    return img[:idx] if idx > 0 else img


def get_image_tag(img: str) -> str | None:
    idx = img.find(":")
    if idx > 0:
        tag = img[idx + 1 :]
    else:
        return None

    return tag if tag != "" else None
