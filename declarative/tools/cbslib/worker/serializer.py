# CBS - workqueue's worker - pydantic model serializer
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

import json
from typing import Any, override
import pydantic


class PydanticSerializer(json.JSONEncoder):
    @override
    def default(self, o: Any) -> Any:  # pyright: ignore[reportExplicitAny, reportAny]
        if isinstance(o, pydantic.BaseModel):
            return o.model_dump()
        else:
            super().default(o)


def pydantic_dumps(obj: Any) -> Any:  # pyright: ignore[reportExplicitAny, reportAny]
    return json.dumps(obj, cls=PydanticSerializer)
