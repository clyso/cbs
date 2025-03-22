# CBS - config - user config
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

from __future__ import annotations

from pathlib import Path

import pydantic
from cbslib.auth.auth import CBSToken
from ceslib.errors import CESError


class CBSUserConfig(pydantic.BaseModel):
    host: str
    login_info: CBSToken

    @classmethod
    def load(cls, path: Path) -> CBSUserConfig:
        if not path.exists():
            raise CESError(f"missing config file  at '{path}'")

        try:
            with path.open("r") as f:
                return CBSUserConfig.model_validate_json(f.read())
        except pydantic.ValidationError:
            raise CESError(f"invalid config at '{path}'")
        except Exception as e:
            raise CESError(f"unexpected error loading config at '{path}': {e}")
