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

from datetime import datetime as dt

import pydantic


class TokenInfo(pydantic.BaseModel):
    user: str
    expires: dt | None


class Token(pydantic.BaseModel):
    token: pydantic.SecretBytes
    info: TokenInfo

    @pydantic.field_serializer("token", when_used="json")
    def dump_secret_token(self, v: pydantic.SecretBytes) -> bytes:
        return v.get_secret_value()
