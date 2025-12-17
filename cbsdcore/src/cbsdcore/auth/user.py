# CBS service daemon core library - auth
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.

from __future__ import annotations

from pathlib import Path

import pydantic
from cbscore.errors import CESError

from cbsdcore.auth.token import Token


class User(pydantic.BaseModel):
    email: str
    name: str
    token: Token


class UserConfig(pydantic.BaseModel):
    host: str
    login_info: Token

    @classmethod
    def load(cls, path: Path) -> UserConfig:
        if not path.exists():
            raise CESError(msg=f"missing config file  at '{path}'")

        try:
            with path.open("r") as f:
                return UserConfig.model_validate_json(f.read())
        except pydantic.ValidationError:
            raise CESError(msg=f"invalid config at '{path}'") from None
        except Exception as e:
            raise CESError(
                msg=f"unexpected error loading config at '{path}': {e}"
            ) from e
